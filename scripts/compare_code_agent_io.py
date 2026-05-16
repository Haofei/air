#!/usr/bin/env python3
"""Run and compare AIR code-agent and OpenCode raw LLM I/O.

The goal of this harness is request-shape alignment, not benchmark scoring. It
captures both systems on the same task, summarizes request/response/tool-call
shape, and writes a machine-readable JSON plus a short Markdown report.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import http.server
import json
import os
import shlex
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


DEFAULT_TASK = (
    "Refactor the todo tool implementation in crates/air-tools/src/lib.rs by "
    "extracting validation of a single todo item from call_todowrite_tool into "
    "a small private helper function. Preserve behavior and run the relevant "
    "air-tools tests."
)
DEFAULT_MODEL_CONFIG = "examples/bigmodel-openai-compatible.json"
DEFAULT_TOOL_CONFIG = "examples/code-agent/tools.json"
DEFAULT_MODIFIED_OPENCODE = Path("/Users/hwang/work/opencode/packages/opencode/src/index.ts")
DEFAULT_BUN = Path("/Users/hwang/.bun/bin/bun")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Compare raw AIR code-agent and OpenCode request/response data."
    )
    subcommands = parser.add_subparsers(dest="command", required=True)

    run_parser = subcommands.add_parser("run", help="run AIR and OpenCode, then analyze")
    add_common_output_args(run_parser)
    run_parser.add_argument("--task", default=DEFAULT_TASK)
    run_parser.add_argument("--repo", type=Path, default=repo_root())
    run_parser.add_argument("--env-file", type=Path, default=repo_root() / ".env")
    run_parser.add_argument("--model-config", default=DEFAULT_MODEL_CONFIG)
    run_parser.add_argument("--tool-config", default=DEFAULT_TOOL_CONFIG)
    run_parser.add_argument("--air-bin", type=Path, default=None)
    run_parser.add_argument("--build-air", action="store_true")
    run_parser.add_argument(
        "--air-copy",
        action="store_true",
        help="run AIR in an isolated copy instead of editing --repo directly",
    )
    run_parser.add_argument(
        "--opencode-command",
        default=default_opencode_command(),
        help=(
            "Command used to invoke OpenCode before its subcommand. Defaults to "
            "the modified local checkout when available."
        ),
    )
    run_parser.add_argument("--opencode-model", default=None)
    run_parser.add_argument("--opencode-agent", default="build")
    run_parser.add_argument(
        "--reuse-opencode-from",
        type=Path,
        default=None,
        help="reuse the OpenCode section from a previous compare run instead of running OpenCode again",
    )
    run_parser.add_argument(
        "--allow-missing-opencode-raw",
        action="store_true",
        help="write a partial report even if OPENCODE_RAW_IO_DIR produced no provider requests",
    )
    run_parser.add_argument("--timeout-seconds", type=int, default=900)
    run_parser.add_argument(
        "--keep-workdirs",
        action="store_true",
        help="keep copied AIR/OpenCode workdirs in the run output directory",
    )
    run_parser.set_defaults(func=run_and_analyze)

    analyze_parser = subcommands.add_parser("analyze", help="analyze existing logs")
    add_common_output_args(analyze_parser)
    analyze_parser.add_argument("--air-trace", type=Path, required=True)
    analyze_parser.add_argument("--air-stderr", type=Path, default=None)
    analyze_parser.add_argument("--air-http-dir", type=Path, default=None)
    analyze_parser.add_argument("--opencode-raw-dir", type=Path, required=True)
    analyze_parser.add_argument("--opencode-http-dir", type=Path, default=None)
    analyze_parser.add_argument("--opencode-events", type=Path, default=None)
    analyze_parser.set_defaults(func=analyze_existing)

    args = parser.parse_args()
    args.func(args)


def add_common_output_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=repo_root() / "target/generated/code-agent-io-compare",
    )
    parser.add_argument("--name", default=None, help="run/report name")


def default_opencode_command() -> str:
    if DEFAULT_BUN.exists() and DEFAULT_MODIFIED_OPENCODE.exists():
        return shlex.join(
            [
                str(DEFAULT_BUN),
                "run",
                "--cwd",
                str(DEFAULT_MODIFIED_OPENCODE.parent.parent),
                "--conditions=browser",
                str(DEFAULT_MODIFIED_OPENCODE),
            ]
        )
    return "opencode"


class HttpCaptureProxy:
    def __init__(self, target_base_url: str, out_dir: Path):
        self.target_base_url = target_base_url.rstrip("/")
        self.out_dir = out_dir
        self._server: http.server.ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None
        self._counter = 0
        self._lock = threading.Lock()
        self.base_url = ""

    def start(self) -> None:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, _format: str, *_args: Any) -> None:
                return

            def do_POST(self) -> None:
                proxy.handle(self)

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

    def next_path(self, kind: str) -> Path:
        with self._lock:
            self._counter += 1
            counter = self._counter
        return self.out_dir / f"{int(time.time() * 1000)}-{counter:04d}-{kind}.json"

    def handle(self, handler: http.server.BaseHTTPRequestHandler) -> None:
        body_bytes = handler.rfile.read(int(handler.headers.get("content-length", "0") or "0"))
        body = parse_json_bytes(body_bytes)
        target_url = self.target_base_url + handler.path
        request_log = {
            "method": handler.command,
            "path": handler.path,
            "url": target_url,
            "headers": redact_headers(dict(handler.headers.items())),
            "body_bytes": len(body_bytes),
            "body": body,
        }
        self.next_path("http-request").write_text(
            json.dumps(request_log, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )

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
                response_body = response.read()
                handler.send_response(response.status)
                for key, value in response.headers.items():
                    if key.lower() in {"content-length", "transfer-encoding", "connection"}:
                        continue
                    handler.send_header(key, value)
                handler.send_header("content-length", str(len(response_body)))
                handler.end_headers()
                handler.wfile.write(response_body)
                self.write_response_log(target_url, response.status, response.headers, response_body)
        except urllib.error.HTTPError as error:
            response_body = error.read()
            handler.send_response(error.code)
            for key, value in error.headers.items():
                if key.lower() in {"content-length", "transfer-encoding", "connection"}:
                    continue
                handler.send_header(key, value)
            handler.send_header("content-length", str(len(response_body)))
            handler.end_headers()
            handler.wfile.write(response_body)
            self.write_response_log(target_url, error.code, error.headers, response_body)
        except urllib.error.URLError as error:
            response_body = json.dumps(
                {"error": f"proxy upstream request failed: {error}"},
                ensure_ascii=False,
            ).encode("utf-8")
            handler.send_response(502)
            handler.send_header("content-type", "application/json")
            handler.send_header("content-length", str(len(response_body)))
            handler.end_headers()
            handler.wfile.write(response_body)
            self.write_response_log(target_url, 502, {}, response_body)

    def write_response_log(self, url: str, status: int, headers: Any, body: bytes) -> None:
        log = {
            "url": url,
            "status": status,
            "headers": redact_headers(dict(headers.items())),
            "body_bytes": len(body),
            "body": parse_json_bytes(body),
        }
        self.next_path("http-response").write_text(
            json.dumps(log, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def parse_json_bytes(data: bytes) -> Any:
    text = data.decode("utf-8", errors="replace")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return text


def redact_headers(headers: dict[str, Any]) -> dict[str, Any]:
    result = {}
    for key, value in headers.items():
        if any(token in key.lower() for token in ["authorization", "api-key", "apikey", "token", "secret"]):
            result[key] = "[REDACTED]"
        else:
            result[key] = value
    return result


def merge_opencode_config_content(existing: str | None, base_url: str) -> str:
    config = {}
    if existing:
        try:
            parsed = json.loads(existing)
            if isinstance(parsed, dict):
                config = parsed
        except json.JSONDecodeError:
            pass
    provider = config.setdefault("provider", {})
    if not isinstance(provider, dict):
        provider = {}
        config["provider"] = provider
    entry = provider.setdefault("zhipuai-coding-plan", {})
    if not isinstance(entry, dict):
        entry = {}
        provider["zhipuai-coding-plan"] = entry
    options = entry.setdefault("options", {})
    if not isinstance(options, dict):
        options = {}
        entry["options"] = options
    options["baseURL"] = base_url
    return json.dumps(config, separators=(",", ":"), ensure_ascii=False)


def run_and_analyze(args: argparse.Namespace) -> None:
    source_repo = args.repo.resolve()
    run_name = args.name or time.strftime("%Y%m%d-%H%M%S")
    run_dir = (args.out_dir / run_name).resolve()
    run_dir.mkdir(parents=True, exist_ok=True)
    reuse_opencode_from = (
        args.reuse_opencode_from.resolve() if args.reuse_opencode_from else None
    )

    opencode_workdir = run_dir / "opencode-work"
    if args.air_copy:
        air_workdir = run_dir / "air-work"
        copy_workspace(source_repo, air_workdir)
        commit_workspace_baseline(air_workdir)
        air_snapshot = None
        air_execution = "isolated-copy"
    else:
        air_workdir = source_repo
        air_snapshot = WorkspaceSnapshot.capture(air_workdir)
        air_execution = "in-place"

    # OpenCode stays isolated. The copy happens before AIR runs so OpenCode does
    # not see or contribute to AIR's in-place refactor.
    if reuse_opencode_from is None:
        copy_workspace(source_repo, opencode_workdir)
        commit_workspace_baseline(opencode_workdir)

    env = os.environ.copy()
    env.update(load_env_file(args.env_file))
    real_base_url = (env.get("OPENAI_BASE_URL") or "").strip()
    air_http_dir = run_dir / "air-http"
    opencode_http_dir = run_dir / "opencode-http"
    capture_http = bool(real_base_url)
    if not capture_http:
        print("[compare-code-agent-io] OPENAI_BASE_URL is not set; HTTP body capture is disabled")

    air_bin = resolve_air_bin(args.air_bin, source_repo, args.build_air)
    air_trace = air_workdir / "target/generated/air.trace.jsonl"
    air_stdout = air_workdir / "target/generated/air.stdout.log"
    air_stderr = air_workdir / "target/generated/air.stderr.log"
    air_trace.parent.mkdir(parents=True, exist_ok=True)

    air_env = env.copy()
    air_proxy = None
    if capture_http:
        air_proxy = HttpCaptureProxy(real_base_url, air_http_dir)
        air_proxy.start()
        air_env["OPENAI_BASE_URL"] = air_proxy.base_url
    try:
        run_checked(
            [
                str(air_bin),
                "code",
                args.task,
                "--model-config",
                str(air_workdir / args.model_config),
                "--tool-config",
                str(air_workdir / args.tool_config),
                "--trace-out",
                str(air_trace.relative_to(air_workdir)),
                "--trace-raw",
                "--log",
            ],
            cwd=air_workdir,
            env=air_env,
            stdout=air_stdout,
            stderr=air_stderr,
            timeout=args.timeout_seconds,
        )
    finally:
        if air_proxy:
            air_proxy.stop()

    if reuse_opencode_from:
        air = analyze_air(
            air_trace,
            stderr_path=air_stderr,
            http_dir=air_http_dir if capture_http else None,
        )
        opencode, opencode_diff = load_reused_opencode_summary(reuse_opencode_from)
        report = {
            "air": air,
            "opencode": opencode,
            "diff": compare_summaries(air, opencode),
        }
        opencode_raw = reuse_opencode_from / "opencode-raw"
        opencode_events = None
        opencode_stderr = None
    else:
        opencode_raw = run_dir / "opencode-raw"
        opencode_raw.mkdir(parents=True, exist_ok=True)
        opencode_events = opencode_workdir / "target/generated/opencode.events.jsonl"
        opencode_stderr = opencode_workdir / "target/generated/opencode.stderr.log"
        opencode_events.parent.mkdir(parents=True, exist_ok=True)
        opencode_env = env.copy()
        opencode_proxy = None
        if capture_http:
            opencode_proxy = HttpCaptureProxy(real_base_url, opencode_http_dir)
            opencode_proxy.start()
        try:
            if opencode_proxy:
                opencode_env["OPENCODE_CONFIG_CONTENT"] = merge_opencode_config_content(
                    opencode_env.get("OPENCODE_CONFIG_CONTENT"),
                    opencode_proxy.base_url,
                )
            opencode_env["OPENCODE_RAW_IO_DIR"] = str(opencode_raw)
            opencode_env["OPENCODE_PROJECT_DIR"] = str(opencode_workdir)
            opencode_env["OPENCODE_PERMISSION"] = json.dumps({"*": "allow"})
            opencode_cmd = [
                *shlex.split(args.opencode_command),
                "run",
                "--format",
                "json",
                "--agent",
                args.opencode_agent,
            ]
            if args.opencode_model:
                opencode_cmd.extend(["--model", args.opencode_model])
            opencode_cmd.append(args.task)
            run_checked(
                opencode_cmd,
                cwd=opencode_workdir,
                env=opencode_env,
                stdout=opencode_events,
                stderr=opencode_stderr,
                timeout=args.timeout_seconds,
            )
            if (
                not list(opencode_raw.glob("*provider-request.json"))
                and not args.allow_missing_opencode_raw
            ):
                raise SystemExit(
                    "OpenCode produced no *provider-request.json files in "
                    f"{opencode_raw}. Use an OpenCode binary built with OPENCODE_RAW_IO_DIR "
                    "support, or pass --allow-missing-opencode-raw for a partial event-only report."
                )
        finally:
            if opencode_proxy:
                opencode_proxy.stop()

        report = analyze_pair(
            air_trace=air_trace,
            air_stderr=air_stderr,
            air_http_dir=air_http_dir if capture_http else None,
            opencode_raw_dir=opencode_raw,
            opencode_http_dir=opencode_http_dir if capture_http else None,
            opencode_events=opencode_events,
        )
        opencode_diff = workspace_diff_summary(opencode_workdir)

    air_diff = (
        workspace_delta_summary(air_workdir, air_snapshot)
        if air_snapshot
        else workspace_diff_summary(air_workdir)
    )
    report["workspace_diff"] = compare_workspace_diff_summaries(air_diff, opencode_diff)
    report["run"] = {
        "name": run_name,
        "task": args.task,
        "execution_order": "sequential: air then reused opencode"
        if reuse_opencode_from
        else "sequential: air then opencode",
        "air_execution": air_execution,
        "air_workdir": str(air_workdir),
        "opencode_workdir": None if reuse_opencode_from else str(opencode_workdir),
        "reuse_opencode_from": str(reuse_opencode_from) if reuse_opencode_from else None,
        "air_trace": str(air_trace),
        "air_http_dir": str(air_http_dir) if capture_http else None,
        "air_stdout": str(air_stdout),
        "air_stderr": str(air_stderr),
        "opencode_raw_dir": str(opencode_raw),
        "opencode_http_dir": None
        if reuse_opencode_from
        else str(opencode_http_dir)
        if capture_http
        else None,
        "opencode_events": str(opencode_events) if opencode_events else None,
        "opencode_stderr": str(opencode_stderr) if opencode_stderr else None,
    }
    write_report(run_dir, report)

    if not args.keep_workdirs:
        if args.air_copy:
            shutil.rmtree(air_workdir, ignore_errors=True)
        if reuse_opencode_from is None:
            shutil.rmtree(opencode_workdir, ignore_errors=True)


def analyze_existing(args: argparse.Namespace) -> None:
    run_name = args.name or "analysis"
    report_dir = (args.out_dir / run_name).resolve()
    report_dir.mkdir(parents=True, exist_ok=True)
    report = analyze_pair(
        air_trace=args.air_trace,
        air_stderr=args.air_stderr,
        air_http_dir=args.air_http_dir,
        opencode_raw_dir=args.opencode_raw_dir,
        opencode_http_dir=args.opencode_http_dir,
        opencode_events=args.opencode_events,
    )
    report["run"] = {
        "name": run_name,
        "execution_order": "analysis-only: existing logs",
        "air_trace": str(args.air_trace),
        "air_stderr": str(args.air_stderr) if args.air_stderr else None,
        "opencode_raw_dir": str(args.opencode_raw_dir),
        "opencode_events": str(args.opencode_events) if args.opencode_events else None,
    }
    write_report(report_dir, report)


def compare_workspace_diff_summaries(
    air: dict[str, Any], opencode: dict[str, Any]
) -> dict[str, Any]:
    return {
        "same_changed_files": air["changed_files"] == opencode["changed_files"],
        "same_diff": air["diff_sha256"] == opencode["diff_sha256"],
        "air": air,
        "opencode": opencode,
    }


def workspace_diff_summary(workdir: Path) -> dict[str, Any]:
    diff = capture_stdout(["git", "diff", "--"], workdir)
    changed_files = capture_stdout(["git", "diff", "--name-only", "--"], workdir).splitlines()
    return {
        "changed_files": changed_files,
        "diff_bytes": len(diff.encode("utf-8")),
        "diff_sha256": hashlib.sha256(diff.encode("utf-8")).hexdigest(),
    }


class WorkspaceSnapshot:
    def __init__(self, files: dict[str, bytes]):
        self.files = files
        self.hashes = {
            path: hashlib.sha256(content).hexdigest() for path, content in files.items()
        }

    @classmethod
    def capture(cls, workdir: Path) -> "WorkspaceSnapshot":
        files: dict[str, bytes] = {}
        for path in workspace_file_list(workdir):
            full_path = workdir / path
            if not full_path.is_file():
                continue
            try:
                files[path] = full_path.read_bytes()
            except OSError:
                continue
        return cls(files)


def workspace_delta_summary(workdir: Path, before: WorkspaceSnapshot) -> dict[str, Any]:
    after = WorkspaceSnapshot.capture(workdir)
    changed_files = sorted(
        path
        for path in set(before.hashes) | set(after.hashes)
        if before.hashes.get(path) != after.hashes.get(path)
    )
    diff = render_snapshot_diff(before, after, changed_files)
    return {
        "changed_files": changed_files,
        "diff_bytes": len(diff.encode("utf-8")),
        "diff_sha256": hashlib.sha256(diff.encode("utf-8")).hexdigest(),
        "diff": diff,
    }


def workspace_file_list(workdir: Path) -> list[str]:
    if (workdir / ".git").exists():
        result = subprocess.run(
            ["git", "ls-files", "-co", "--exclude-standard", "-z"],
            cwd=workdir,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode == 0:
            return [
                path
                for path in result.stdout.decode("utf-8", errors="replace").split("\0")
                if path and not ignored_workspace_path(path)
            ]
    paths = []
    for path in workdir.rglob("*"):
        if path.is_file():
            relative = path.relative_to(workdir).as_posix()
            if not ignored_workspace_path(relative):
                paths.append(relative)
    return sorted(paths)


def ignored_workspace_path(path: str) -> bool:
    return path.startswith(
        (
            ".git/",
            ".air/",
            "target/",
            "node_modules/",
            "__pycache__/",
        )
    )


def render_snapshot_diff(
    before: WorkspaceSnapshot, after: WorkspaceSnapshot, changed_files: list[str]
) -> str:
    hunks: list[str] = []
    for path in changed_files:
        old = before.files.get(path)
        new = after.files.get(path)
        if old is not None and new is None:
            old_text = decode_diff_text(old)
            if old_text is None:
                hunks.append(f"Binary file deleted: {path}")
                continue
            new_lines: list[str] = []
            old_lines = old_text.splitlines()
        elif old is None and new is not None:
            new_text = decode_diff_text(new)
            if new_text is None:
                hunks.append(f"Binary file added: {path}")
                continue
            old_lines = []
            new_lines = new_text.splitlines()
        elif old is not None and new is not None:
            old_text = decode_diff_text(old)
            new_text = decode_diff_text(new)
            if old_text is None or new_text is None:
                hunks.append(f"Binary file changed: {path}")
                continue
            old_lines = old_text.splitlines()
            new_lines = new_text.splitlines()
        else:
            continue
        hunks.extend(
            difflib.unified_diff(
                old_lines,
                new_lines,
                fromfile=f"a/{path}",
                tofile=f"b/{path}",
                lineterm="",
            )
        )
    return "\n".join(hunks)


def decode_diff_text(content: bytes) -> str | None:
    if b"\0" in content:
        return None
    try:
        return content.decode("utf-8")
    except UnicodeDecodeError:
        return content.decode("utf-8", errors="replace")


def analyze_pair(
    air_trace: Path,
    air_stderr: Path | None,
    air_http_dir: Path | None,
    opencode_raw_dir: Path,
    opencode_http_dir: Path | None,
    opencode_events: Path | None,
) -> dict[str, Any]:
    air = analyze_air(air_trace, stderr_path=air_stderr, http_dir=air_http_dir)
    opencode = analyze_opencode(opencode_raw_dir, opencode_events, http_dir=opencode_http_dir)
    return {
        "air": air,
        "opencode": opencode,
        "diff": compare_summaries(air, opencode),
    }


def load_reused_opencode_summary(run_dir: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    summary_path = run_dir / "summary.json"
    if not summary_path.exists():
        raise SystemExit(f"Cannot reuse OpenCode results: {summary_path} does not exist")
    summary = read_json(summary_path)
    opencode = summary.get("opencode")
    if not isinstance(opencode, dict):
        raise SystemExit(f"Cannot reuse OpenCode results: {summary_path} has no opencode section")
    workspace = summary.get("workspace_diff") if isinstance(summary.get("workspace_diff"), dict) else {}
    opencode_diff = workspace.get("opencode") if isinstance(workspace.get("opencode"), dict) else None
    if opencode_diff is None:
        opencode_diff = {
            "changed_files": [],
            "diff_bytes": 0,
            "diff_sha256": "",
        }
    return opencode, opencode_diff


def analyze_air(trace_path: Path, stderr_path: Path | None, http_dir: Path | None) -> dict[str, Any]:
    events = read_jsonl(trace_path)
    requests: list[dict[str, Any]] = []
    responses: list[dict[str, Any]] = []
    for event in events:
        if (
            event.get("agent") == "code-edit-loop-agent"
            and event.get("action") == "model_call"
            and event.get("status") == "ok"
            and (event.get("meta") or {}).get("model") == "code_edit_decider"
        ):
            meta = event.get("meta") or {}
            request = meta.get("provider_request") or {}
            requests.append(summarize_air_request(meta, request))
            responses.append(summarize_air_response(event))
    http_requests = summarize_http_requests(http_dir)
    display_requests = http_requests or requests

    tool_sequence = []
    for event in events:
        if (
            event.get("agent") == "code-edit-loop-agent"
            and event.get("action") == "tool_batch_dispatch_item"
            and event.get("status") in {"ok", "error"}
        ):
            meta = event.get("meta") or {}
            tool_sequence.append(
                {
                    "tool": meta.get("tool"),
                    "status": event.get("status"),
                    "input": compact_value(event.get("input")),
                }
            )

    first_request = requests[0] if requests else {}
    final_output = next(
        (
            event.get("output")
            for event in reversed(events)
            if event.get("agent") == "code-edit-loop-agent"
            and event.get("action") == "return"
        ),
        None,
    )
    return {
        "trace": str(trace_path),
        "stderr": str(stderr_path) if stderr_path else None,
        "http_dir": str(http_dir) if http_dir else None,
        "request_source": "http" if http_requests else "trace",
        "request_count": len(display_requests),
        "requests": display_requests,
        "trace_requests": requests,
        "http_requests": http_requests,
        "responses": responses,
        "first_request": display_requests[0] if display_requests else {},
        "tool_sequence": tool_sequence,
        "tool_counts": count_tools(tool_sequence),
        "first_edit_tool_index": first_tool_index(tool_sequence, {"edit", "write"}),
        "empty_glob_calls": [
            item for item in tool_sequence if item["tool"] == "glob" and item.get("input") == {}
        ],
        "final": summarize_final(final_output),
    }


def summarize_air_request(meta: dict[str, Any], request: dict[str, Any]) -> dict[str, Any]:
    messages = request.get("messages") if isinstance(request.get("messages"), list) else []
    tools = request.get("tools") if isinstance(request.get("tools"), list) else []
    trace_strings = json.dumps(request, ensure_ascii=False)
    return {
        "bytes": meta.get("provider_request_bytes"),
        "input_bytes": meta.get("input_bytes"),
        "provider_user_content_bytes": meta.get("provider_user_content_bytes"),
        "provider_tools_bytes": meta.get("provider_tools_bytes"),
        "trace_request_truncated": "[AIR_TRUNCATED]" in trace_strings,
        "message_count": len(messages),
        "roles": [message.get("role") for message in messages],
        "message_content_shapes": [content_shape(message.get("content")) for message in messages],
        "tools": [tool.get("function", {}).get("name") for tool in tools],
        "tool_schema": summarize_openai_tools(tools),
    }


def summarize_air_response(event: dict[str, Any]) -> dict[str, Any]:
    output = event.get("output") if isinstance(event.get("output"), dict) else {}
    assistant = output.get("_air_assistant") if isinstance(output.get("_air_assistant"), dict) else {}
    tool_calls = output.get("tool_calls") if isinstance(output.get("tool_calls"), list) else []
    return {
        "complete": output.get("complete"),
        "text_preview": first_text(assistant),
        "tool_calls": summarize_decision_tool_calls(tool_calls),
    }


def analyze_opencode(raw_dir: Path, events_path: Path | None, http_dir: Path | None) -> dict[str, Any]:
    request_paths = sorted(raw_dir.glob("*provider-request.json"))
    response_paths = sorted(raw_dir.glob("*provider-response-stream.json"))
    sdk_requests = [summarize_opencode_request(path) for path in request_paths]
    http_requests = summarize_http_requests(http_dir)
    requests = http_requests or sdk_requests
    responses = [summarize_opencode_response(path) for path in response_paths]
    tool_sequence = parse_opencode_tool_sequence(events_path) if events_path else []
    first_request = requests[0] if requests else {}
    return {
        "raw_dir": str(raw_dir),
        "http_dir": str(http_dir) if http_dir else None,
        "events": str(events_path) if events_path else None,
        "request_source": "http" if http_requests else "sdk",
        "request_count": len(requests),
        "requests": requests,
        "sdk_requests": sdk_requests,
        "http_requests": http_requests,
        "responses": responses,
        "first_request": first_request,
        "tool_sequence": tool_sequence,
        "tool_counts": count_tools(tool_sequence),
        "first_edit_tool_index": first_tool_index(tool_sequence, {"edit", "write"}),
        "empty_glob_calls": [
            item for item in tool_sequence if item["tool"] == "glob" and item.get("input") == {}
        ],
    }


def summarize_opencode_request(path: Path) -> dict[str, Any]:
    wrapper = read_json(path)
    params = wrapper.get("params") if isinstance(wrapper.get("params"), dict) else wrapper
    prompt = params.get("prompt") if isinstance(params.get("prompt"), list) else []
    tools = params.get("tools") if isinstance(params.get("tools"), list) else []
    return {
        "path": str(path),
        "bytes": path.stat().st_size,
        "provider_id": wrapper.get("providerID"),
        "model_id": wrapper.get("modelID"),
        "agent": wrapper.get("agent"),
        "prompt_count": len(prompt),
        "roles": [message.get("role") for message in prompt],
        "message_content_shapes": [content_shape(message.get("content")) for message in prompt],
        "tools_bytes": json_bytes(tools),
        "tools": [tool.get("name") for tool in tools],
        "tool_schema": summarize_ai_sdk_tools(tools),
    }


def summarize_http_requests(http_dir: Path | None) -> list[dict[str, Any]]:
    if not http_dir or not http_dir.exists():
        return []
    return [summarize_http_request(path) for path in sorted(http_dir.glob("*-http-request.json"))]


def summarize_http_request(path: Path) -> dict[str, Any]:
    wrapper = read_json(path)
    body = wrapper.get("body") if isinstance(wrapper.get("body"), dict) else {}
    messages = body.get("messages") if isinstance(body.get("messages"), list) else []
    tools = body.get("tools") if isinstance(body.get("tools"), list) else []
    return {
        "path": str(path),
        "source": "http",
        "url": wrapper.get("url"),
        "method": wrapper.get("method"),
        "bytes": wrapper.get("body_bytes") or json_bytes(body),
        "provider_user_content_bytes": provider_user_content_bytes_from_messages(messages),
        "provider_tools_bytes": json_bytes(tools),
        "message_count": len(messages),
        "roles": [message.get("role") for message in messages],
        "message_content_shapes": [content_shape(message.get("content")) for message in messages],
        "tools": [tool.get("function", {}).get("name") for tool in tools],
        "tool_schema": summarize_openai_tools(tools),
        "top_keys": sorted(body.keys()),
    }


def summarize_opencode_response(path: Path) -> dict[str, Any]:
    data = read_json(path)
    chunks = data.get("chunks") if isinstance(data.get("chunks"), list) else []
    reasoning_parts: list[str] = []
    text_parts: list[str] = []
    tool_inputs: dict[str, dict[str, str]] = {}
    for chunk in chunks:
        if not isinstance(chunk, dict):
            continue
        kind = chunk.get("type")
        if kind == "reasoning-delta":
            reasoning_parts.append(str(chunk.get("delta", "")))
        elif kind == "text-delta":
            text_parts.append(str(chunk.get("delta", "")))
        elif kind == "tool-input-start":
            call_id = str(chunk.get("id", ""))
            tool_inputs[call_id] = {"tool": str(chunk.get("toolName", "")), "input": ""}
        elif kind == "tool-input-delta":
            call_id = str(chunk.get("id", ""))
            tool_inputs.setdefault(call_id, {"tool": "", "input": ""})["input"] += str(
                chunk.get("delta", "")
            )
    return {
        "path": str(path),
        "reasoning_preview": truncate("".join(reasoning_parts), 500),
        "text_preview": truncate("".join(text_parts), 500),
        "tool_calls": [
            {"tool": value["tool"], "input": parse_json_maybe(value["input"])}
            for value in tool_inputs.values()
        ],
    }


def compare_summaries(air: dict[str, Any], opencode: dict[str, Any]) -> dict[str, Any]:
    air_first = air.get("first_request") or {}
    open_first = opencode.get("first_request") or {}
    air_tools = set(air_first.get("tools") or [])
    open_tools = set(open_first.get("tools") or [])
    shared_tools = sorted(air_tools & open_tools)
    schema_diff = {}
    for tool in shared_tools:
        left = (air_first.get("tool_schema") or {}).get(tool, {})
        right = (open_first.get("tool_schema") or {}).get(tool, {})
        delta = {}
        for key in ["required", "properties", "additional_properties"]:
            if left.get(key) != right.get(key):
                delta[key] = {"air": left.get(key), "opencode": right.get(key)}
        if left.get("description_len") != right.get("description_len"):
            delta["description_len"] = {
                "air": left.get("description_len"),
                "opencode": right.get("description_len"),
            }
        if delta:
            schema_diff[tool] = delta

    return {
        "request_count": {
            "air": air.get("request_count"),
            "opencode": opencode.get("request_count"),
        },
        "request_source": {
            "air": air.get("request_source"),
            "opencode": opencode.get("request_source"),
        },
        "first_request_bytes": {
            "air": air_first.get("bytes"),
            "opencode": open_first.get("bytes"),
        },
        "first_top_keys": {
            "air": air_first.get("top_keys"),
            "opencode": open_first.get("top_keys"),
        },
        "request_bytes": {
            "air": [request.get("bytes") for request in air.get("requests", [])],
            "opencode": [request.get("bytes") for request in opencode.get("requests", [])],
        },
        "air_trace_request_truncated": any(
            request.get("trace_request_truncated") for request in air.get("requests", [])
        ),
        "message_roles_first_request": {
            "air": air_first.get("roles"),
            "opencode": open_first.get("roles"),
        },
        "message_content_shapes_first_request": {
            "air": air_first.get("message_content_shapes"),
            "opencode": open_first.get("message_content_shapes"),
        },
        "tools": {
            "air": sorted(air_tools),
            "opencode": sorted(open_tools),
            "air_only": sorted(air_tools - open_tools),
            "opencode_only": sorted(open_tools - air_tools),
            "shared": shared_tools,
        },
        "schema_diff": schema_diff,
        "tool_counts": {
            "air": air.get("tool_counts"),
            "opencode": opencode.get("tool_counts"),
        },
        "first_edit_tool_index": {
            "air": air.get("first_edit_tool_index"),
            "opencode": opencode.get("first_edit_tool_index"),
        },
        "empty_glob_calls": {
            "air": len(air.get("empty_glob_calls") or []),
            "opencode": len(opencode.get("empty_glob_calls") or []),
        },
    }


def summarize_openai_tools(tools: list[Any]) -> dict[str, Any]:
    summary = {}
    for tool in tools:
        function = tool.get("function") if isinstance(tool, dict) else {}
        if not isinstance(function, dict):
            continue
        name = function.get("name")
        if not name:
            continue
        schema = function.get("parameters") if isinstance(function.get("parameters"), dict) else {}
        summary[name] = summarize_schema(function.get("description", ""), schema)
    return summary


def summarize_ai_sdk_tools(tools: list[Any]) -> dict[str, Any]:
    summary = {}
    for tool in tools:
        if not isinstance(tool, dict) or not tool.get("name"):
            continue
        schema = tool.get("inputSchema") if isinstance(tool.get("inputSchema"), dict) else {}
        summary[tool["name"]] = summarize_schema(tool.get("description", ""), schema)
    return summary


def summarize_schema(description: str, schema: dict[str, Any]) -> dict[str, Any]:
    properties = schema.get("properties") if isinstance(schema.get("properties"), dict) else {}
    return {
        "description_len": len(description or ""),
        "required": schema.get("required") if isinstance(schema.get("required"), list) else [],
        "properties": sorted(properties.keys()),
        "additional_properties": schema.get("additionalProperties"),
        "nested": {
            name: summarize_nested_schema(value)
            for name, value in properties.items()
            if isinstance(value, dict)
        },
    }


def summarize_nested_schema(schema: dict[str, Any]) -> dict[str, Any]:
    item = schema.get("items") if isinstance(schema.get("items"), dict) else None
    properties = schema.get("properties") if isinstance(schema.get("properties"), dict) else {}
    result = {
        "required": schema.get("required") if isinstance(schema.get("required"), list) else [],
        "properties": sorted(properties.keys()),
        "additional_properties": schema.get("additionalProperties"),
    }
    if item:
        result["items"] = summarize_nested_schema(item)
    return result


def parse_opencode_tool_sequence(events_path: Path) -> list[dict[str, Any]]:
    if not events_path.exists():
        return []
    sequence = []
    for event in read_jsonl(events_path):
        if event.get("type") != "tool_use":
            continue
        part = event.get("part") if isinstance(event.get("part"), dict) else {}
        state = part.get("state") if isinstance(part.get("state"), dict) else {}
        sequence.append(
            {
                "tool": part.get("tool"),
                "status": state.get("status"),
                "input": compact_value(state.get("input")),
            }
        )
    return sequence


def write_report(run_dir: Path, report: dict[str, Any]) -> None:
    summary_json = run_dir / "summary.json"
    summary_md = run_dir / "summary.md"
    summary_json.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    summary_md.write_text(render_markdown(report), encoding="utf-8")
    print(f"[compare-code-agent-io] wrote {summary_json}")
    print(f"[compare-code-agent-io] wrote {summary_md}")


def render_markdown(report: dict[str, Any]) -> str:
    diff = report["diff"]
    run = report.get("run", {})
    lines = [
        "# Code Agent I/O Comparison",
        "",
        "## Execution",
        "",
        f"- Order: `{run.get('execution_order', 'unknown')}`",
        "",
        "## Request Shape",
        "",
        f"- Request count: AIR `{diff['request_count']['air']}`, OpenCode `{diff['request_count']['opencode']}`",
        f"- Request source: AIR `{diff['request_source']['air']}`, OpenCode `{diff['request_source']['opencode']}`",
        f"- First request bytes: AIR `{diff['first_request_bytes']['air']}`, OpenCode `{diff['first_request_bytes']['opencode']}`",
        f"- First top-level keys: AIR `{diff['first_top_keys']['air']}`, OpenCode `{diff['first_top_keys']['opencode']}`",
        f"- AIR bytes: `{diff['request_bytes']['air']}`",
        f"- OpenCode bytes: `{diff['request_bytes']['opencode']}`",
        f"- AIR trace request truncated: `{diff['air_trace_request_truncated']}`",
        f"- First roles: AIR `{diff['message_roles_first_request']['air']}`, OpenCode `{diff['message_roles_first_request']['opencode']}`",
        f"- First content shapes: AIR `{diff['message_content_shapes_first_request']['air']}`, OpenCode `{diff['message_content_shapes_first_request']['opencode']}`",
        "",
        "## Tools",
        "",
        f"- AIR tools: `{diff['tools']['air']}`",
        f"- OpenCode tools: `{diff['tools']['opencode']}`",
        f"- AIR-only: `{diff['tools']['air_only']}`",
        f"- OpenCode-only: `{diff['tools']['opencode_only']}`",
        "",
        "## Tool Behavior",
        "",
        f"- Tool counts: AIR `{diff['tool_counts']['air']}`, OpenCode `{diff['tool_counts']['opencode']}`",
        f"- First edit tool index: AIR `{diff['first_edit_tool_index']['air']}`, OpenCode `{diff['first_edit_tool_index']['opencode']}`",
        f"- Empty glob calls: AIR `{diff['empty_glob_calls']['air']}`, OpenCode `{diff['empty_glob_calls']['opencode']}`",
        "",
    ]
    if "workspace_diff" in report:
        workspace = report["workspace_diff"]
        lines.extend(
            [
                "## Workspace Diff",
                "",
                f"- Same changed files: `{workspace['same_changed_files']}`",
                f"- Same diff hash: `{workspace['same_diff']}`",
                f"- AIR changed files: `{workspace['air']['changed_files']}`",
                f"- OpenCode changed files: `{workspace['opencode']['changed_files']}`",
                f"- AIR diff bytes: `{workspace['air']['diff_bytes']}`",
                f"- OpenCode diff bytes: `{workspace['opencode']['diff_bytes']}`",
                "",
            ]
        )
    lines.extend(["## Schema Diffs", ""])
    if diff["schema_diff"]:
        for tool, tool_diff in diff["schema_diff"].items():
            lines.append(f"### `{tool}`")
            lines.append("")
            lines.append("```json")
            lines.append(json.dumps(tool_diff, indent=2, ensure_ascii=False))
            lines.append("```")
            lines.append("")
    else:
        lines.append("No schema diffs for shared tools.")
        lines.append("")

    lines.extend(
        [
            "## Paths",
            "",
            "```json",
            json.dumps(report.get("run", {}), indent=2, ensure_ascii=False),
            "```",
            "",
        ]
    )
    return "\n".join(lines)


def copy_workspace(source: Path, target: Path) -> None:
    if target.exists():
        shutil.rmtree(target)
    target.parent.mkdir(parents=True, exist_ok=True)
    run_checked(
        [
            "rsync",
            "-a",
            "--delete",
            "--exclude",
            "target",
            "--exclude",
            ".air",
            "--exclude",
            ".env",
            "--exclude",
            ".env.*",
            "--exclude",
            "__pycache__",
            "--exclude",
            "*.pyc",
            f"{source}/",
            f"{target}/",
        ],
        cwd=source,
        env=os.environ.copy(),
        stdout=None,
        stderr=None,
        timeout=300,
    )


def commit_workspace_baseline(workdir: Path) -> None:
    if not (workdir / ".git").exists():
        return
    env = os.environ.copy()
    env["GIT_AUTHOR_NAME"] = "AIR Compare"
    env["GIT_AUTHOR_EMAIL"] = "air-compare@example.invalid"
    env["GIT_COMMITTER_NAME"] = "AIR Compare"
    env["GIT_COMMITTER_EMAIL"] = "air-compare@example.invalid"
    env["HUSKY"] = "0"
    run_checked(
        ["git", "add", "-A"],
        cwd=workdir,
        env=env,
        stdout=None,
        stderr=None,
        timeout=300,
    )
    run_checked(
        [
            "git",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--no-verify",
            "-m",
            "air compare baseline",
        ],
        cwd=workdir,
        env=env,
        stdout=None,
        stderr=None,
        timeout=300,
    )


def resolve_air_bin(air_bin: Path | None, source_repo: Path, build: bool) -> Path:
    candidate = air_bin or (source_repo / "target/debug/air")
    if build or not candidate.exists():
        run_checked(
            ["cargo", "build", "-p", "air-cli"],
            cwd=source_repo,
            env=os.environ.copy(),
            stdout=None,
            stderr=None,
            timeout=600,
        )
    if not candidate.exists():
        raise SystemExit(f"AIR binary not found: {candidate}")
    return candidate.resolve()


def run_checked(
    command: list[str],
    cwd: Path,
    env: dict[str, str],
    stdout: Path | None,
    stderr: Path | None,
    timeout: int,
) -> None:
    stdout_handle = stdout.open("w", encoding="utf-8") if stdout else None
    stderr_handle = stderr.open("w", encoding="utf-8") if stderr else None
    try:
        print(f"[compare-code-agent-io] $ {' '.join(shlex.quote(part) for part in command)}")
        result = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            stdout=stdout_handle,
            stderr=stderr_handle,
            timeout=timeout,
            check=False,
        )
    finally:
        if stdout_handle:
            stdout_handle.close()
        if stderr_handle:
            stderr_handle.close()
    if result.returncode != 0:
        raise SystemExit(
            f"command failed with exit {result.returncode}: "
            f"{' '.join(shlex.quote(part) for part in command)}"
        )


def capture_stdout(command: list[str], cwd: Path) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"command failed with exit {result.returncode}: "
            f"{' '.join(shlex.quote(part) for part in command)}\n{result.stderr}"
        )
    return result.stdout


def read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    events = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError as error:
                raise SystemExit(f"{path}:{line_number}: invalid JSONL: {error}") from error
            if isinstance(value, dict):
                events.append(value)
    return events


def load_env_file(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    result = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        result[key.strip()] = value.strip().strip('"').strip("'")
    return result


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def content_shape(content: Any) -> str:
    if isinstance(content, str):
        return f"string:{len(content)}"
    if isinstance(content, list):
        types = [item.get("type") for item in content if isinstance(item, dict)]
        return f"array:{len(content)}:{types}"
    if content is None:
        return "null"
    return type(content).__name__


def json_bytes(value: Any) -> int:
    return len(json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8"))


def provider_user_content_bytes_from_messages(messages: list[Any]) -> int:
    total = 0
    for message in messages:
        if not isinstance(message, dict) or message.get("role") != "user":
            continue
        content = message.get("content")
        if isinstance(content, str):
            total += len(content.encode("utf-8"))
        else:
            total += json_bytes(content)
    return total


def compact_value(value: Any) -> Any:
    if isinstance(value, dict):
        return {key: compact_value(value[key]) for key in sorted(value.keys())}
    if isinstance(value, list):
        return [compact_value(item) for item in value]
    if isinstance(value, str):
        return truncate(value, 1_000)
    return value


def truncate(text: str, max_chars: int) -> str:
    if len(text) <= max_chars:
        return text
    return text[: max_chars - 32] + f"...[truncated {len(text) - max_chars + 32} chars]"


def parse_json_maybe(value: str) -> Any:
    try:
        return json.loads(value)
    except Exception:
        return truncate(value, 1_000)


def first_text(assistant: dict[str, Any]) -> str | None:
    for key in ["reasoning", "content", "answer"]:
        value = assistant.get(key)
        if isinstance(value, str) and value.strip():
            return truncate(value.strip(), 500)
    return None


def summarize_decision_tool_calls(tool_calls: list[Any]) -> list[dict[str, Any]]:
    result = []
    for item in tool_calls:
        if not isinstance(item, dict):
            continue
        result.append({"tool": item.get("tool"), "input": compact_value(item.get("input"))})
    return result


def count_tools(sequence: list[dict[str, Any]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for item in sequence:
        tool = item.get("tool")
        if tool:
            counts[tool] = counts.get(tool, 0) + 1
    return dict(sorted(counts.items()))


def first_tool_index(sequence: list[dict[str, Any]], names: set[str]) -> int | None:
    for index, item in enumerate(sequence, start=1):
        if item.get("tool") in names:
            return index
    return None


def summarize_final(output: Any) -> dict[str, Any]:
    if not isinstance(output, dict):
        return {}
    return {
        "final_success": output.get("final_success"),
        "patch_applied": output.get("patch_applied"),
        "changed_files": output.get("changed_files"),
    }


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
