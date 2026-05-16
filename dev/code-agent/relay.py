#!/usr/bin/env python3
"""Replay captured code-agent HTTP calls and optionally resume with a live provider.

This tool consumes the HTTP capture directories produced by
dev/code-agent/compare.py. It is intentionally transport-level: replay is
based on call order, while request shape differences are logged for diagnosis.
"""

from __future__ import annotations

import argparse
import http.server
import json
import os
import shlex
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

sys.dont_write_bytecode = True

DEFAULT_SIDE = "air"
MAX_LATE_ACTION_ROUTES = 2
MAX_TOLERATED_LATE_TARGETED_INSPECTIONS = 2
MAX_REFERENCE_EXTRA_MODEL_CALLS = 4


def main() -> None:
    argv, child_command = split_child_command(sys.argv[1:])
    parser = argparse.ArgumentParser(
        description="Inspect or replay captured AIR/OpenCode model HTTP calls."
    )
    subcommands = parser.add_subparsers(dest="command", required=True)

    inspect_parser = subcommands.add_parser("inspect", help="summarize a captured run")
    add_capture_args(inspect_parser)
    inspect_parser.set_defaults(func=inspect_run)

    diff_parser = subcommands.add_parser(
        "diff", help="compare AIR and OpenCode request/response at one call"
    )
    diff_parser.add_argument("run_dir", type=Path, help="compare run directory")
    diff_parser.add_argument("--call", type=int, required=True, help="1-based model call number")
    diff_parser.add_argument(
        "--context",
        type=int,
        default=6,
        help="how many trailing request messages to show for each side",
    )
    diff_parser.set_defaults(func=diff_run)

    divergence_parser = subcommands.add_parser(
        "divergence", help="find likely AIR/OpenCode divergence points"
    )
    divergence_parser.add_argument("run_dir", type=Path, help="compare run directory")
    divergence_parser.add_argument(
        "--window",
        type=int,
        default=3,
        help="timeline window around the first tool divergence",
    )
    divergence_parser.set_defaults(func=divergence_run)

    serve_parser = subcommands.add_parser(
        "serve", help="start a local replay/forwarding OpenAI-compatible relay"
    )
    add_capture_args(serve_parser)
    add_relay_args(serve_parser)
    serve_parser.set_defaults(func=serve_relay)

    run_parser = subcommands.add_parser(
        "run",
        help="run a command with OPENAI_BASE_URL pointed at a relay",
        epilog="Put the child command after --, for example: run CAPTURE --from 23 -- cargo run ...",
    )
    add_capture_args(run_parser)
    add_relay_args(run_parser)
    run_parser.add_argument(
        "--env-file",
        type=Path,
        default=repo_root() / ".env",
        help="environment file loaded before the child command",
    )
    run_parser.set_defaults(func=run_with_relay)

    args = parser.parse_args(argv)
    if args.command == "run":
        args.child_command = child_command
    args.func(args)


def split_child_command(argv: list[str]) -> tuple[list[str], list[str]]:
    if not argv or argv[0] != "run":
        return argv, []
    try:
        separator = argv.index("--")
    except ValueError:
        return argv, []
    return argv[:separator], argv[separator + 1 :]


def add_capture_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("run_dir", type=Path, help="compare run directory or *-http directory")
    parser.add_argument(
        "--side",
        choices=["air", "opencode"],
        default=DEFAULT_SIDE,
        help="which captured side to use",
    )


def add_relay_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--from",
        dest="from_call",
        type=int,
        default=None,
        help=(
            "1-based call number where live forwarding begins. Calls before it "
            "are replayed. Defaults to replaying the whole capture."
        ),
    )
    parser.add_argument(
        "--target-base-url",
        default=None,
        help="live upstream base URL. Defaults to the base URL in the captured requests.",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=None,
        help="where relay request/response logs are written",
    )


@dataclass
class CapturedCall:
    index: int
    request_path: Path
    response_path: Path | None
    request: dict[str, Any]
    response: dict[str, Any] | None


class RouteMonitor:
    def __init__(self, reference: list[CapturedCall], label: str = "OpenCode"):
        self.reference = reference
        self.label = label
        self.stop_reason: dict[str, Any] | None = None
        self.reference_cursor = 0
        self.first_reference_action = first_action_route_call(reference)
        self.air_action_seen = False
        self.late_action_routes = 0
        self.tolerated_late_targeted_inspections = 0

    def stop_before(self, call_index: int) -> dict[str, Any] | None:
        if (
            not self.stop_reason
            and self.reference
            and call_index > len(self.reference) + MAX_REFERENCE_EXTRA_MODEL_CALLS
        ):
            self.stop_reason = {
                "kind": "reference_call_budget_exceeded",
                "call_index": call_index,
                "reference_calls": len(self.reference),
                "max_extra_calls": MAX_REFERENCE_EXTRA_MODEL_CALLS,
                "message": (
                    f"AIR reached model call {call_index}, exceeding {self.label}'s "
                    f"{len(self.reference)} calls by more than {MAX_REFERENCE_EXTRA_MODEL_CALLS}"
                ),
            }
        return self.stop_reason

    def observe_response(
        self, call_index: int, body: Any, *, enforce: bool = True
    ) -> dict[str, Any] | None:
        if self.stop_reason:
            return self.stop_reason
        actual = route_body_tool_signature(body)
        if not actual:
            return None
        actual_has_action = route_has_action(actual)
        if actual_has_action:
            self.air_action_seen = True
        expected_call = self.peek_reference_call()
        if expected_call is None:
            stop_reason = {
                "kind": "extra_tool_route",
                "call_index": call_index,
                "actual_tools": actual,
                "reference_calls": len(self.reference),
                "message": (
                    f"AIR emitted tool route {actual} at call {call_index}, but "
                    f"{self.label} has no remaining non-todo tool route"
                ),
            }
            if enforce:
                self.stop_reason = stop_reason
            return self.stop_reason
        expected = route_tool_signature(expected_call)
        expected_has_action = route_has_action(expected)
        if self.air_action_seen:
            if expected_has_action and tool_route_compatible(expected, actual):
                self.consume_reference_call(expected_call)
            return None
        if not actual_has_action and not expected_has_action:
            self.consume_reference_call(expected_call)
            return None
        if (
            not actual_has_action
            and self.first_reference_action is not None
            and not self.air_action_seen
            and expected_has_action
        ):
            if is_targeted_inspection_route(body):
                self.tolerated_late_targeted_inspections += 1
                if (
                    self.tolerated_late_targeted_inspections
                    <= MAX_TOLERATED_LATE_TARGETED_INSPECTIONS
                ):
                    return None
            self.late_action_routes += 1
            if self.late_action_routes < MAX_LATE_ACTION_ROUTES:
                return None
            stop_reason = {
                "kind": "late_action",
                "call_index": call_index,
                "reference_action_call_index": self.first_reference_action.index,
                "expected_tools": route_tool_signature(self.first_reference_action),
                "actual_tools": actual,
                "message": (
                    f"{self.label} reached an edit/write route at call "
                    f"{self.first_reference_action.index}, but AIR was still using "
                    f"{actual} at call {call_index}"
                ),
            }
            if enforce:
                self.stop_reason = stop_reason
            return self.stop_reason
        if actual_has_action and not expected_has_action:
            return None
        if not tool_route_compatible(expected, actual):
            stop_reason = {
                "kind": "tool_signature_divergence",
                "call_index": call_index,
                "expected_call_index": expected_call.index,
                "expected_tools": expected,
                "actual_tools": actual,
                "message": (
                    f"AIR tool route diverged from {self.label} at call {call_index} "
                    f"(reference call {expected_call.index}): expected {expected}, got {actual}"
                ),
            }
            if enforce:
                self.stop_reason = stop_reason
            return self.stop_reason
        self.consume_reference_call(expected_call)
        return self.stop_reason

    def peek_reference_call(self) -> CapturedCall | None:
        cursor = self.reference_cursor
        while cursor < len(self.reference):
            call = self.reference[cursor]
            cursor += 1
            if route_tool_signature(call):
                return call
        return None

    def consume_reference_call(self, call: CapturedCall) -> None:
        self.reference_cursor = max(self.reference_cursor, call.index)


def inspect_run(args: argparse.Namespace) -> None:
    cassette = load_cassette(args.run_dir, args.side)
    print(render_inspection(cassette))


def diff_run(args: argparse.Namespace) -> None:
    air = load_cassette(args.run_dir, "air")
    opencode = load_cassette(args.run_dir, "opencode")
    print(render_call_diff(air, opencode, args.call, args.context))


def divergence_run(args: argparse.Namespace) -> None:
    air = load_cassette(args.run_dir, "air")
    opencode = load_cassette(args.run_dir, "opencode")
    print(render_divergence_report(args.run_dir, air, opencode, args.window))


def serve_relay(args: argparse.Namespace) -> None:
    cassette = load_cassette(args.run_dir, args.side)
    relay = build_relay(args, cassette)
    relay.start()
    try:
        print(render_relay_banner(relay, cassette), flush=True)
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        print("\n[code-agent-relay] stopping", flush=True)
    finally:
        relay.stop()


def run_with_relay(args: argparse.Namespace) -> None:
    child_command = list(args.child_command)
    if child_command and child_command[0] == "--":
        child_command = child_command[1:]
    if not child_command:
        raise SystemExit("run requires a child command after --")

    cassette = load_cassette(args.run_dir, args.side)
    relay = build_relay(args, cassette)
    env = load_env(args.env_file)
    relay.start()
    env["OPENAI_BASE_URL"] = relay.base_url
    try:
        print(render_relay_banner(relay, cassette), flush=True)
        print(
            f"[code-agent-relay] running: {shlex.join(child_command)}",
            flush=True,
        )
        raise SystemExit(
            run_child(
                child_command,
                env=env,
                cwd=repo_root(),
                stop_when=relay.rate_limit_message,
            )
        )
    finally:
        relay.stop()


def run_child(
    command: list[str],
    *,
    env: dict[str, str],
    cwd: Path,
    stop_when: Callable[[], str | None] | None = None,
) -> int:
    process = subprocess.Popen(command, env=env, cwd=cwd)
    while True:
        returncode = process.poll()
        if returncode is not None:
            if stop_when:
                stop_reason = stop_when()
                if stop_reason:
                    raise SystemExit(stop_reason)
            return int(returncode)
        if stop_when:
            stop_reason = stop_when()
            if stop_reason:
                process.kill()
                process.wait()
                raise SystemExit(stop_reason)
        time.sleep(1)


def build_relay(args: argparse.Namespace, cassette: list[CapturedCall]) -> "ReplayRelay":
    replay_until = len(cassette) + 1 if args.from_call is None else args.from_call
    if replay_until < 1:
        raise SystemExit("--from must be >= 1")
    target_base_url = args.target_base_url or infer_target_base_url(cassette)
    log_dir = args.out_dir or default_relay_log_dir(args.run_dir, args.side)
    return ReplayRelay(
        cassette=cassette,
        replay_until=replay_until,
        target_base_url=target_base_url,
        out_dir=log_dir,
    )


class ReplayRelay:
    def __init__(
        self,
        *,
        cassette: list[CapturedCall],
        replay_until: int,
        target_base_url: str | None,
        out_dir: Path,
        monitor_reference: list[CapturedCall] | None = None,
        path_rewrites: list[tuple[str, str]] | None = None,
    ):
        self.cassette = cassette
        self.replay_until = replay_until
        self.target_base_url = target_base_url.rstrip("/") if target_base_url else None
        self.out_dir = out_dir
        self._server: http.server.ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None
        self._lock = threading.Lock()
        self._counter = 0
        self._call_index = 0
        self.base_url = ""
        self.monitor = RouteMonitor(monitor_reference) if monitor_reference else None
        self.path_rewrites = path_rewrites or []
        self.rate_limit_stop: dict[str, Any] | None = None

    def start(self) -> None:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        relay = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, _format: str, *_args: Any) -> None:
                return

            def do_POST(self) -> None:
                relay.handle(self)

        self._server = http.server.ThreadingHTTPServer(("127.0.0.1", free_port()), Handler)
        host, port = self._server.server_address
        self.base_url = f"http://{host}:{port}"
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    def stop(self) -> None:
        if self._server:
            self._server.shutdown()
            self._server.server_close()
        if self._thread:
            self._thread.join(timeout=5)

    def handle(self, handler: http.server.BaseHTTPRequestHandler) -> None:
        body_bytes = handler.rfile.read(int(handler.headers.get("content-length", "0") or "0"))
        body = parse_json_bytes(body_bytes)
        call_index = self.next_call_index()
        target_url = self.target_url(handler.path)
        request_log = {
            "call_index": call_index,
            "mode": "replay" if call_index < self.replay_until else "forward",
            "method": handler.command,
            "path": handler.path,
            "url": target_url,
            "headers": redact_headers(dict(handler.headers.items())),
            "body_bytes": len(body_bytes),
            "body": body,
            "shape": request_shape(body),
        }
        cassette_call = self.cassette[call_index - 1] if call_index <= len(self.cassette) else None
        if cassette_call:
            request_log["cassette_shape"] = request_shape(cassette_call.request.get("body"))
            request_log["shape_delta"] = shape_delta(
                request_log["shape"],
                request_log["cassette_shape"],
            )
        self.write_json("http-request", request_log)
        if self.rate_limit_stop:
            self.send_json_error(
                handler,
                429,
                "relay stopped after upstream HTTP 429",
                call_index,
            )
            return

        if call_index < self.replay_until:
            self.replay_response(handler, call_index)
            return
        if self.monitor:
            stop_reason = self.monitor.stop_before(call_index)
            if stop_reason:
                self.send_json_error(handler, 409, stop_reason["message"], call_index)
                return
        self.forward_request(handler, body_bytes, target_url, call_index)

    def replay_response(self, handler: http.server.BaseHTTPRequestHandler, call_index: int) -> None:
        if call_index > len(self.cassette):
            self.send_json_error(
                handler,
                502,
                f"no captured response for replay call {call_index}",
                call_index,
            )
            return
        captured = self.cassette[call_index - 1]
        if not captured.response:
            self.send_json_error(
                handler,
                502,
                f"captured call {call_index} has no response",
                call_index,
            )
            return

        status = int(captured.response.get("status") or 200)
        headers = captured.response.get("headers") or {}
        captured_body = rewrite_json_strings(captured.response.get("body"), self.path_rewrites)
        response_body = encode_captured_body(captured_body)
        parsed_body = parse_json_bytes(response_body)
        if self.monitor and status < 400:
            self.monitor.observe_response(call_index, parsed_body, enforce=False)
        self.send_response(handler, status, headers, response_body)
        self.write_json(
            "http-response",
            {
                "call_index": call_index,
                "mode": "replay",
                "replayed_from": str(captured.response_path),
                "status": status,
                "headers": redact_headers(headers),
                "body_bytes": len(response_body),
                "body": parsed_body,
            },
        )
        print(f"[code-agent-relay] call {call_index}: replayed", flush=True)

    def forward_request(
        self,
        handler: http.server.BaseHTTPRequestHandler,
        body_bytes: bytes,
        target_url: str | None,
        call_index: int,
    ) -> None:
        if not target_url:
            self.send_json_error(
                handler,
                502,
                "live forwarding needs --target-base-url or a captured request URL",
                call_index,
            )
            return
        headers = {
            key: value
            for key, value in handler.headers.items()
            if key.lower() not in {"host", "content-length", "accept-encoding", "connection"}
        }
        request = urllib.request.Request(
            target_url,
            data=body_bytes,
            headers=headers,
            method=handler.command,
        )
        try:
            with urllib.request.urlopen(request, timeout=600) as response:
                request_body = parse_json_bytes(body_bytes)
                if isinstance(request_body, dict) and request_body.get("stream") is True:
                    response_body = self.forward_stream_response(
                        handler, response, call_index, target_url
                    )
                else:
                    response_body = response.read()
                    self.send_response(
                        handler, response.status, dict(response.headers.items()), response_body
                    )
                self.write_forward_response(call_index, target_url, response.status, response.headers, response_body)
        except urllib.error.HTTPError as error:
            response_body = error.read()
            self.record_rate_limit(call_index, target_url, error.code, response_body)
            self.send_response(handler, error.code, dict(error.headers.items()), response_body)
            self.write_forward_response(call_index, target_url, error.code, error.headers, response_body)
        except urllib.error.URLError as error:
            self.send_json_error(handler, 502, f"upstream request failed: {error}", call_index)
            return
        print(f"[code-agent-relay] call {call_index}: forwarded -> {target_url}", flush=True)

    def forward_stream_response(
        self,
        handler: http.server.BaseHTTPRequestHandler,
        response: Any,
        call_index: int,
        target_url: str,
    ) -> bytes:
        chunks = bytearray()
        progress_path = self.next_path("http-stream-progress")
        last_progress_at = time.time()
        handler.send_response(response.status)
        for key, value in response.headers.items():
            if key.lower() in {"content-length", "transfer-encoding", "connection"}:
                continue
            handler.send_header(key, value)
        handler.end_headers()
        while True:
            chunk = response.readline()
            if not chunk:
                break
            chunks.extend(chunk)
            handler.wfile.write(chunk)
            handler.wfile.flush()
            now = time.time()
            if len(chunks) <= len(chunk) or now - last_progress_at >= 5:
                write_stream_progress(progress_path, call_index, target_url, bytes(chunks))
                print(
                    f"[code-agent-relay] stream call {call_index}: received {len(chunks)} bytes "
                    f"(progress: {progress_path})",
                    flush=True,
                )
                last_progress_at = now
        write_stream_progress(progress_path, call_index, target_url, bytes(chunks), done=True)
        return bytes(chunks)

    def write_forward_response(
        self,
        call_index: int,
        target_url: str,
        status: int,
        headers: Any,
        response_body: bytes,
    ) -> None:
        parsed_body = parse_json_bytes(response_body)
        stop_reason = (
            self.monitor.observe_response(call_index, parsed_body)
            if self.monitor and status < 400
            else None
        )
        self.write_json(
            "http-response",
            {
                "call_index": call_index,
                "mode": "forward",
                "url": target_url,
                "status": status,
                "headers": redact_headers(dict(headers.items())),
                "body_bytes": len(response_body),
                "body": parsed_body,
                "route_monitor": stop_reason,
            },
        )
        if stop_reason:
            print(f"[code-agent-relay] route divergence detected: {stop_reason['message']}", flush=True)

    def record_rate_limit(
        self, call_index: int, target_url: str, status: int, body: bytes
    ) -> None:
        if status != 429 or self.rate_limit_stop:
            return
        self.rate_limit_stop = {
            "kind": "upstream_rate_limited",
            "call_index": call_index,
            "url": target_url,
            "status": status,
            "body": parse_json_bytes(body),
        }
        print(
            f"[code-agent-relay] upstream returned HTTP 429 at model call {call_index}; stopping",
            flush=True,
        )

    def rate_limit_message(self) -> str | None:
        if not self.rate_limit_stop:
            return None
        call_index = self.rate_limit_stop.get("call_index")
        return f"upstream returned HTTP 429 at model call {call_index}; stopping run"

    def send_json_error(
        self,
        handler: http.server.BaseHTTPRequestHandler,
        status: int,
        message: str,
        call_index: int,
    ) -> None:
        body = json.dumps({"error": message}, ensure_ascii=False).encode("utf-8")
        handler.send_response(status)
        handler.send_header("content-type", "application/json")
        handler.send_header("content-length", str(len(body)))
        handler.end_headers()
        handler.wfile.write(body)
        self.write_json(
            "http-response",
            {
                "call_index": call_index,
                "mode": "error",
                "status": status,
                "headers": {"content-type": "application/json"},
                "body_bytes": len(body),
                "body": parse_json_bytes(body),
            },
        )
        print(f"[code-agent-relay] call {call_index}: error: {message}", flush=True)

    def send_response(
        self,
        handler: http.server.BaseHTTPRequestHandler,
        status: int,
        headers: dict[str, Any],
        body: bytes,
    ) -> None:
        handler.send_response(status)
        for key, value in headers.items():
            if key.lower() in {
                "content-length",
                "transfer-encoding",
                "connection",
                "server",
                "date",
            }:
                continue
            handler.send_header(key, str(value))
        if not any(key.lower() == "content-type" for key in headers):
            handler.send_header("content-type", "application/json")
        handler.send_header("content-length", str(len(body)))
        handler.end_headers()
        handler.wfile.write(body)

    def target_url(self, path: str) -> str | None:
        if not self.target_base_url:
            return None
        return self.target_base_url + path

    def next_call_index(self) -> int:
        with self._lock:
            self._call_index += 1
            return self._call_index

    def next_path(self, kind: str) -> Path:
        with self._lock:
            self._counter += 1
            counter = self._counter
        return self.out_dir / f"{int(time.time() * 1000)}-{counter:04d}-{kind}.json"

    def write_json(self, kind: str, value: dict[str, Any]) -> None:
        self.next_path(kind).write_text(
            json.dumps(value, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )


def load_cassette(path: Path, side: str) -> list[CapturedCall]:
    http_dir = resolve_http_dir(path.resolve(), side)
    request_paths = sorted(http_dir.glob("*http-request.json"))
    response_paths = sorted(http_dir.glob("*http-response.json"))
    if not request_paths:
        raise SystemExit(f"no *http-request.json files found in {http_dir}")
    calls: list[CapturedCall] = []
    for index, request_path in enumerate(request_paths, start=1):
        response_path = response_paths[index - 1] if index <= len(response_paths) else None
        calls.append(
            CapturedCall(
                index=index,
                request_path=request_path,
                response_path=response_path,
                request=read_json_file(request_path),
                response=read_json_file(response_path) if response_path else None,
            )
        )
    return calls


def resolve_http_dir(path: Path, side: str) -> Path:
    if path.name.endswith("-http"):
        return path
    candidate = path / f"{side}-http"
    if candidate.exists():
        return candidate
    summary_path = path / "summary.json"
    if summary_path.exists():
        summary = read_json_file(summary_path)
        run = summary.get("run") if isinstance(summary.get("run"), dict) else {}
        explicit_path = run.get(f"{side}_http_dir")
        if isinstance(explicit_path, str) and explicit_path:
            explicit = Path(explicit_path).expanduser()
            if explicit.exists():
                return explicit.resolve()
        if side == "opencode":
            reused = run.get("reuse_opencode_from")
            if isinstance(reused, str) and reused:
                reused_candidate = Path(reused).expanduser() / "opencode-http"
                if reused_candidate.exists():
                    return reused_candidate.resolve()
            source_run = run.get("source_run")
            if isinstance(source_run, str) and source_run:
                return resolve_http_dir(Path(source_run).expanduser().resolve(), side)
    raise SystemExit(f"expected {candidate} or a direct *-http directory")


def render_inspection(cassette: list[CapturedCall]) -> str:
    http_dir = cassette[0].request_path.parent
    request_bytes = [int(call.request.get("body_bytes") or 0) for call in cassette]
    response_bytes = [
        int(call.response.get("body_bytes") or 0)
        for call in cassette
        if call.response
    ]
    shapes = [request_shape(call.request.get("body")) for call in cassette]
    tool_names = count_response_tool_calls(cassette)
    first_write = first_tool_call(cassette, {"edit", "write"})
    target = infer_target_base_url(cassette) or "(unknown)"
    lines = [
        f"[code-agent-relay] http_dir: {http_dir}",
        f"target_base_url: {target}",
        f"calls: requests={len(cassette)} responses={sum(1 for call in cassette if call.response)}",
        f"request_bytes: min={min(request_bytes)} max={max(request_bytes)} median={median(request_bytes)}",
    ]
    if response_bytes:
        lines.append(
            f"response_bytes: min={min(response_bytes)} max={max(response_bytes)} median={median(response_bytes)}"
        )
    lines.append(
        "request_shape: "
        f"models={sorted({shape.get('model') for shape in shapes if shape.get('model')})} "
        f"tools={sorted({shape.get('tools') for shape in shapes if shape.get('tools') is not None})} "
        f"stream={sorted({shape.get('stream') for shape in shapes})}"
    )
    lines.append(f"first_edit_or_write_call: {first_write or '(none observed)'}")
    if tool_names:
        top = ", ".join(f"{name}={count}" for name, count in sorted(tool_names.items(), key=lambda item: (-item[1], item[0]))[:12])
        lines.append(f"assistant_tool_calls: {top}")
    return "\n".join(lines)


def render_relay_banner(relay: ReplayRelay, cassette: list[CapturedCall]) -> str:
    replayed = max(0, min(len(cassette), relay.replay_until - 1))
    live = "disabled" if not relay.target_base_url else relay.target_base_url
    monitor = getattr(relay, "monitor", None)
    return "\n".join(
        [
            f"[code-agent-relay] listening: {relay.base_url}",
            f"[code-agent-relay] replaying calls: 1..{replayed}",
            f"[code-agent-relay] forwarding from call: {relay.replay_until}",
            f"[code-agent-relay] live target: {live}",
            f"[code-agent-relay] log dir: {relay.out_dir}",
            f"[code-agent-relay] route monitor: {monitor.label if monitor else 'disabled'}",
        ]
    )


def render_call_diff(
    air: list[CapturedCall],
    opencode: list[CapturedCall],
    call_index: int,
    context_messages: int,
) -> str:
    if call_index < 1:
        raise SystemExit("--call must be >= 1")
    air_call = air[call_index - 1] if call_index <= len(air) else None
    opencode_call = opencode[call_index - 1] if call_index <= len(opencode) else None
    if not air_call and not opencode_call:
        raise SystemExit(
            f"call {call_index} is outside both captures "
            f"(air={len(air)}, opencode={len(opencode)})"
        )

    lines = [
        f"# Model Call {call_index}",
        "",
        "## Request Shape",
        "",
        render_shape_row("AIR", air_call),
        render_shape_row("OpenCode", opencode_call),
        "",
        "## Last Request Messages",
        "",
        "### AIR",
        "",
        *render_request_messages(air_call, context_messages),
        "",
        "### OpenCode",
        "",
        *render_request_messages(opencode_call, context_messages),
        "",
        "## Assistant Response",
        "",
        "### AIR",
        "",
        *render_response_summary(air_call),
        "",
        "### OpenCode",
        "",
        *render_response_summary(opencode_call),
    ]
    return "\n".join(lines)


def _build_commands_section(
    run_dir: Path,
    first_tool: int | None,
    suggested_from: int,
    action_from: int | None,
) -> list[str]:
    cmd_lines = [
        "",
        "## Commands",
        "",
        "```bash",
        f"python3 dev/code-agent/relay.py diff {run_dir} --call {first_tool or suggested_from}",
        (
            f"python3 dev/code-agent/compare.py replay-air {run_dir} "
            f"--from {suggested_from} --name replay-from-{suggested_from}"
        ),
    ]
    if action_from is not None:
        cmd_lines.append(
            f"python3 dev/code-agent/compare.py replay-air {run_dir} "
            f"--from {action_from} --name replay-action-from-{action_from}"
        )
    cmd_lines.extend(["```", ""])
    return cmd_lines


def render_divergence_report(
    run_dir: Path,
    air: list[CapturedCall],
    opencode: list[CapturedCall],
    window: int,
) -> str:
    first_tool = first_tool_divergence(air, opencode)
    first_size = first_request_size_divergence(air, opencode, threshold=0.25)
    air_action = first_action_call(air)
    opencode_action = first_action_call(opencode)
    suggested_from = max(1, (first_tool or first_size or 1) - 1)
    action_from = max(1, opencode_action[0] - 1) if opencode_action else None

    lines = [
        "# Divergence Report",
        "",
        f"- run: `{run_dir}`",
        f"- AIR calls: `{len(air)}`",
        f"- OpenCode calls: `{len(opencode)}`",
        f"- first tool divergence: `{first_tool or 'none'}`",
        f"- first request-size divergence (>25%): `{first_size or 'none'}`",
        f"- AIR first edit/write: `{format_action_call(air_action)}`",
        f"- OpenCode first edit/write: `{format_action_call(opencode_action)}`",
        f"- suggested replay: `--from {suggested_from}`",
    ]
    if action_from is not None:
        lines.append(f"- action-focused replay: `--from {action_from}`")
    lines.extend(_build_commands_section(run_dir, first_tool, suggested_from, action_from))
    lines.extend(["## Timeline", ""])
    timeline_at = first_tool or first_size or 1
    lines.extend(render_divergence_timeline(air, opencode, timeline_at, window))
    return "\n".join(lines)


def first_tool_divergence(air: list[CapturedCall], opencode: list[CapturedCall]) -> int | None:
    opencode_cursor = 0
    for call in air:
        actual = route_tool_signature(call)
        if not actual:
            continue
        expected_call = next_route_call(opencode, opencode_cursor)
        if expected_call is None:
            return call.index
        opencode_cursor = expected_call.index
        expected = route_tool_signature(expected_call)
        if not tool_route_compatible(expected, actual):
            return call.index
    if next_route_call(opencode, opencode_cursor) is not None:
        return len(air) + 1
    return None


def next_route_call(cassette: list[CapturedCall], cursor: int) -> CapturedCall | None:
    while cursor < len(cassette):
        call = cassette[cursor]
        cursor += 1
        if route_tool_signature(call):
            return call
    return None


def first_request_size_divergence(
    air: list[CapturedCall], opencode: list[CapturedCall], threshold: float
) -> int | None:
    for index in range(1, min(len(air), len(opencode)) + 1):
        air_bytes = int(air[index - 1].request.get("body_bytes") or 0)
        open_bytes = int(opencode[index - 1].request.get("body_bytes") or 0)
        baseline = max(air_bytes, open_bytes, 1)
        if abs(air_bytes - open_bytes) / baseline > threshold:
            return index
    return None


def first_action_call(cassette: list[CapturedCall]) -> tuple[int, str] | None:
    for call in cassette:
        for name in response_tool_signature(call):
            if name in {"edit", "write"}:
                return (call.index, name)
    return None


def first_action_route_call(cassette: list[CapturedCall]) -> CapturedCall | None:
    for call in cassette:
        if route_has_action(route_tool_signature(call)):
            return call
    return None


def format_action_call(value: tuple[int, str] | None) -> str:
    if value is None:
        return "none"
    return f"{value[0]}:{value[1]}"


def render_divergence_timeline(
    air: list[CapturedCall],
    opencode: list[CapturedCall],
    center: int,
    window: int,
) -> list[str]:
    start = max(1, center - window)
    end = min(max(len(air), len(opencode)), center + window)
    lines = []
    for index in range(start, end + 1):
        air_call = air[index - 1] if index <= len(air) else None
        open_call = opencode[index - 1] if index <= len(opencode) else None
        marker = ">>" if index == center else "  "
        lines.append(
            f"{marker} {index:02d} AIR={format_call_brief(air_call)} | "
            f"OpenCode={format_call_brief(open_call)}"
        )
    return lines


def format_call_brief(call: CapturedCall | None) -> str:
    if call is None:
        return "none"
    tools = [summarize_tool_call_name(name) for name in response_tool_signature(call)]
    text = assistant_response((call.response or {}).get("body")).get("text") or ""
    if not text:
        text = assistant_response((call.response or {}).get("body")).get("reasoning") or ""
    return (
        f"bytes={call.request.get('body_bytes')} "
        f"tools={tools or []} text={shorten(text, 100) or '(empty)'}"
    )


def response_tool_signature(call: CapturedCall) -> list[str]:
    return body_tool_signature((call.response or {}).get("body"))


def route_tool_signature(call: CapturedCall) -> list[str]:
    return route_body_tool_signature((call.response or {}).get("body"))


def route_body_tool_signature(body: Any) -> list[str]:
    return route_relevant_tools(body_tool_signature(body))


def route_body_tool_calls(body: Any) -> list[dict[str, Any]]:
    response = assistant_response(body)
    calls = []
    for tool_call in response.get("tool_calls") or []:
        name = tool_call.get("name")
        if not name or name in {"todowrite", "todoread"}:
            continue
        calls.append(tool_call)
    return calls


def route_relevant_tools(tools: list[str]) -> list[str]:
    ignored = {"todowrite", "todoread"}
    return [tool for tool in tools if tool not in ignored]


def is_targeted_inspection_route(body: Any) -> bool:
    calls = route_body_tool_calls(body)
    if len(calls) != 1:
        return False
    name = calls[0].get("name")
    arguments = parse_tool_arguments(calls[0].get("arguments"))
    if not isinstance(arguments, dict):
        return False
    if name == "read":
        if not any(arguments.get(key) for key in ("filePath", "path")):
            return False
        return any(
            key in arguments
            for key in (
                "offset",
                "limit",
                "start_line",
                "end_line",
                "startLine",
                "endLine",
            )
        )
    if name in {"grep", "glob"}:
        return bool(arguments.get("pattern"))
    if name == "bash":
        command = str(arguments.get("command") or "").strip()
        return command.startswith("rg ") or command.startswith("git grep ")
    return False


def parse_tool_arguments(arguments: Any) -> Any:
    if isinstance(arguments, str):
        try:
            return json.loads(arguments)
        except json.JSONDecodeError:
            return None
    return arguments


def route_has_action(tools: list[str]) -> bool:
    return any(tool in {"edit", "write"} for tool in tools)


def tool_route_compatible(expected: list[str], actual: list[str]) -> bool:
    if expected == actual:
        return True
    if not expected:
        return not any(tool in {"edit", "write"} for tool in actual)
    cursor = 0
    for tool in actual:
        if cursor < len(expected) and tool == expected[cursor]:
            cursor += 1
    return cursor == len(expected)


def body_tool_signature(body: Any) -> list[str]:
    response = assistant_response(body)
    return [
        str(tool_call.get("name"))
        for tool_call in response.get("tool_calls") or []
        if tool_call.get("name")
    ]


def summarize_tool_call_name(name: str) -> str:
    return name


def render_shape_row(label: str, call: CapturedCall | None) -> str:
    if call is None:
        return f"- {label}: no call"
    shape = request_shape(call.request.get("body"))
    return (
        f"- {label}: bytes={call.request.get('body_bytes')} "
        f"messages={shape.get('messages')} tools={shape.get('tools')} "
        f"stream={shape.get('stream')} response_bytes="
        f"{(call.response or {}).get('body_bytes')}"
    )


def render_request_messages(call: CapturedCall | None, count: int) -> list[str]:
    if call is None:
        return ["- no call"]
    body = call.request.get("body") if isinstance(call.request.get("body"), dict) else {}
    messages = body.get("messages") if isinstance(body.get("messages"), list) else []
    if not messages:
        return ["- no messages"]
    lines: list[str] = []
    first = max(0, len(messages) - count)
    for index, message in enumerate(messages[first:], start=first + 1):
        if not isinstance(message, dict):
            continue
        lines.append(f"- {index}. {summarize_message(message)}")
    return lines


def summarize_message(message: dict[str, Any]) -> str:
    role = message.get("role") or "unknown"
    parts = [f"role={role}"]
    if message.get("tool_call_id"):
        parts.append(f"tool_call_id={message.get('tool_call_id')}")
    content = message.get("content")
    if isinstance(content, str) and content:
        parts.append(f"content={shorten(content, 500)}")
    elif isinstance(content, list):
        parts.append(f"content=list[{len(content)}]")
    if isinstance(message.get("tool_calls"), list):
        calls = []
        for tool_call in message["tool_calls"]:
            function = tool_call.get("function") if isinstance(tool_call, dict) else {}
            if isinstance(function, dict):
                calls.append(summarize_tool_call(function.get("name"), function.get("arguments")))
        parts.append("tool_calls=[" + "; ".join(calls) + "]")
    return " ".join(parts)


def render_response_summary(call: CapturedCall | None) -> list[str]:
    if call is None:
        return ["- no call"]
    if not call.response:
        return ["- no response"]
    response = assistant_response(call.response.get("body"))
    lines = [
        f"- status: {call.response.get('status')}",
        f"- text: {shorten(response.get('text') or '', 800) or '(empty)'}",
    ]
    reasoning = response.get("reasoning") or ""
    if reasoning:
        lines.append(f"- reasoning: {shorten(reasoning, 800)}")
    tools = response.get("tool_calls") or []
    if tools:
        lines.append("- tool_calls:")
        for tool_call in tools:
            lines.append(
                f"  - {summarize_tool_call(tool_call.get('name'), tool_call.get('arguments'))}"
            )
    else:
        lines.append("- tool_calls: []")
    return lines


def assistant_response(body: Any) -> dict[str, Any]:
    if isinstance(body, dict):
        choice = (body.get("choices") or [{}])[0]
        message = choice.get("message") if isinstance(choice.get("message"), dict) else {}
        return {
            "text": message.get("content") or "",
            "reasoning": message.get("reasoning_content") or "",
            "tool_calls": [
                {
                    "name": (tool_call.get("function") or {}).get("name"),
                    "arguments": (tool_call.get("function") or {}).get("arguments"),
                }
                for tool_call in message.get("tool_calls") or []
                if isinstance(tool_call, dict)
            ],
        }
    if isinstance(body, str):
        return stream_assistant_response(body)
    return {"text": "", "reasoning": "", "tool_calls": []}


def stream_assistant_response(text: str) -> dict[str, Any]:
    content_parts: list[str] = []
    reasoning_parts: list[str] = []
    calls: dict[int, dict[str, str]] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("data:"):
            continue
        payload = line[5:].strip()
        if not payload or payload == "[DONE]":
            continue
        try:
            event = json.loads(payload)
        except json.JSONDecodeError:
            continue
        for choice in event.get("choices") or []:
            delta = choice.get("delta") or {}
            if delta.get("content"):
                content_parts.append(str(delta["content"]))
            if delta.get("reasoning_content"):
                reasoning_parts.append(str(delta["reasoning_content"]))
            for tool_call in delta.get("tool_calls") or []:
                index = int(tool_call.get("index") or 0)
                function = tool_call.get("function") or {}
                current = calls.setdefault(index, {"name": "", "arguments": ""})
                current["name"] += str(function.get("name") or "")
                current["arguments"] += str(function.get("arguments") or "")
    return {
        "text": "".join(content_parts),
        "reasoning": "".join(reasoning_parts),
        "tool_calls": list(calls.values()),
    }


def summarize_tool_call(name: Any, arguments: Any) -> str:
    parsed = parse_json_maybe(arguments) if isinstance(arguments, str) else arguments
    detail = ""
    if isinstance(parsed, dict):
        if name in {"read", "edit", "write"}:
            detail = str(
                parsed.get("filePath")
                or parsed.get("file_path")
                or parsed.get("path")
                or ""
            )
            if name == "read":
                offset = parsed.get("offset", "")
                limit = parsed.get("limit", "")
                if offset != "" or limit != "":
                    detail += f":{offset}+{limit}"
        elif name in {"grep", "bash"}:
            detail = str(parsed.get("pattern") or parsed.get("command") or "")
        elif name == "todowrite":
            detail = f"todos={len(parsed.get('todos') or [])}"
        else:
            detail = json.dumps(parsed, ensure_ascii=False)
    elif parsed:
        detail = str(parsed)
    return f"{name}({shorten(detail, 240)})" if detail else str(name)


def infer_task_from_cassette(cassette: list[CapturedCall]) -> str | None:
    if not cassette:
        return None
    body = cassette[0].request.get("body")
    if not isinstance(body, dict):
        return None
    messages = body.get("messages") if isinstance(body.get("messages"), list) else []
    for message in messages:
        if not isinstance(message, dict) or message.get("role") != "user":
            continue
        content = message.get("content")
        if isinstance(content, str) and content.strip():
            return unquote_json_string(content.strip())
    return None


def unquote_json_string(value: str) -> str:
    if value.startswith('"') and value.endswith('"'):
        try:
            parsed = json.loads(value)
            if isinstance(parsed, str):
                return parsed
        except json.JSONDecodeError:
            pass
    return value


def request_shape(body: Any) -> dict[str, Any]:
    if not isinstance(body, dict):
        return {"kind": type(body).__name__}
    messages = body.get("messages") if isinstance(body.get("messages"), list) else []
    tools = body.get("tools") if isinstance(body.get("tools"), list) else []
    return {
        "model": body.get("model"),
        "messages": len(messages),
        "tools": len(tools),
        "stream": body.get("stream", False),
        "tool_choice": summarize_tool_choice(body.get("tool_choice")),
        "max_tokens": body.get("max_tokens"),
        "bytes_estimate": len(json.dumps(body, ensure_ascii=False).encode("utf-8")),
    }


def summarize_tool_choice(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        return str(value.get("type") or value.get("function") or "dict")
    if value is None:
        return "none"
    return type(value).__name__


def shape_delta(current: dict[str, Any], captured: dict[str, Any]) -> dict[str, Any]:
    delta = {}
    for key in sorted(set(current) | set(captured)):
        if current.get(key) != captured.get(key):
            delta[key] = {"current": current.get(key), "captured": captured.get(key)}
    return delta


def count_response_tool_calls(cassette: list[CapturedCall]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for call in cassette:
        for name in response_tool_names(call.response.get("body") if call.response else None):
            counts[name] = counts.get(name, 0) + 1
    return counts


def first_tool_call(cassette: list[CapturedCall], names: set[str]) -> str | None:
    for call in cassette:
        for name in response_tool_names(call.response.get("body") if call.response else None):
            if name in names:
                return f"{call.index}:{name}"
    return None


def response_tool_names(body: Any) -> list[str]:
    if isinstance(body, dict):
        names: list[str] = []
        for choice in body.get("choices") or []:
            message = choice.get("message") or {}
            for tool_call in message.get("tool_calls") or []:
                function = tool_call.get("function") or {}
                name = function.get("name")
                if isinstance(name, str):
                    names.append(name)
        return names
    if isinstance(body, str):
        return stream_response_tool_names(body)
    return []


def stream_response_tool_names(text: str) -> list[str]:
    names: list[str] = []
    fragments: dict[int, str] = {}
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("data:"):
            continue
        payload = line[5:].strip()
        if not payload or payload == "[DONE]":
            continue
        try:
            event = json.loads(payload)
        except json.JSONDecodeError:
            continue
        for choice in event.get("choices") or []:
            delta = choice.get("delta") or {}
            for tool_call in delta.get("tool_calls") or []:
                index = int(tool_call.get("index") or 0)
                function = tool_call.get("function") or {}
                name = function.get("name")
                if isinstance(name, str):
                    fragments[index] = fragments.get(index, "") + name
    for name in fragments.values():
        if name:
            names.append(name)
    return names


def infer_target_base_url(cassette: list[CapturedCall]) -> str | None:
    for call in cassette:
        url = call.request.get("url")
        path = call.request.get("path")
        if not isinstance(url, str) or not isinstance(path, str):
            continue
        if url.endswith(path):
            return url[: -len(path)].rstrip("/")
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme and parsed.netloc:
            return f"{parsed.scheme}://{parsed.netloc}"
    return None


def default_relay_log_dir(run_dir: Path, side: str) -> Path:
    base = run_dir.resolve()
    if base.name.endswith("-http"):
        base = base.parent
    return base / f"relay-{side}-http"


def encode_captured_body(body: Any) -> bytes:
    if isinstance(body, str):
        return body.encode("utf-8")
    return json.dumps(body, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def rewrite_json_strings(value: Any, rewrites: list[tuple[str, str]]) -> Any:
    if not rewrites:
        return value
    if isinstance(value, str):
        rewritten = value
        for old, new in rewrites:
            if old:
                rewritten = rewritten.replace(old, new)
        return rewritten
    if isinstance(value, list):
        return [rewrite_json_strings(item, rewrites) for item in value]
    if isinstance(value, dict):
        return {
            key: rewrite_json_strings(item, rewrites)
            for key, item in value.items()
        }
    return value


def read_json_file(path: Path | None) -> dict[str, Any] | None:
    if path is None:
        return None
    return json.loads(path.read_text(encoding="utf-8"))


def parse_json_bytes(data: bytes) -> Any:
    text = data.decode("utf-8", errors="replace")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return text


def write_stream_progress(
    path: Path, call_index: int, url: str, body: bytes, *, done: bool = False
) -> None:
    text = body.decode("utf-8", errors="replace")
    assistant = assistant_response(text)
    progress = {
        "call_index": call_index,
        "url": url,
        "done": done,
        "body_bytes": len(body),
        "assistant": {
            "content_chars": len(assistant.get("text") or ""),
            "reasoning_chars": len(assistant.get("reasoning") or ""),
            "tool_calls": [
                {
                    "name": tool_call.get("name"),
                    "arguments_chars": len(str(tool_call.get("arguments") or "")),
                }
                for tool_call in assistant.get("tool_calls") or []
            ],
        },
    }
    path.write_text(json.dumps(progress, indent=2, ensure_ascii=False), encoding="utf-8")


def parse_json_maybe(value: str) -> Any:
    try:
        return json.loads(value)
    except Exception:
        return value


def shorten(value: str, max_chars: int) -> str:
    value = " ".join(value.split())
    if len(value) <= max_chars:
        return value
    return value[: max_chars - 24] + f"...[{len(value) - max_chars + 24} chars]"


def redact_headers(headers: dict[str, Any]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in headers.items():
        lowered = key.lower()
        if any(token in lowered for token in ["authorization", "api-key", "apikey", "token", "secret"]):
            result[key] = "[REDACTED]"
        else:
            result[key] = value
    return result


def median(values: list[int]) -> int:
    ordered = sorted(values)
    return ordered[len(ordered) // 2]


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def load_env_file(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    values: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def load_env(path: Path) -> dict[str, str]:
    values = load_env_file(path)
    values.update(os.environ)
    return values


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


if __name__ == "__main__":
    main()
