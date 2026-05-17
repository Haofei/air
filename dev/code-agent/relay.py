#!/usr/bin/env python3
"""Inspect captured code-agent HTTP calls and request/response divergence.

This tool consumes the HTTP capture directories produced by
dev/code-agent/compare.py. AIR replay now lives in `air code --replay-artifact`;
this script is only for analyzing raw provider I/O.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.parse
from dataclasses import dataclass
from pathlib import Path
from typing import Any

sys.dont_write_bytecode = True

DEFAULT_SIDE = "air"
MAX_LATE_ACTION_ROUTES = 2
MAX_TOLERATED_LATE_TARGETED_INSPECTIONS = 2
MAX_REFERENCE_EXTRA_MODEL_CALLS = 4
ACTION_TOOLS = {"apply_patch", "edit", "write"}


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Inspect captured AIR/OpenCode model HTTP calls."
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

    args = parser.parse_args()
    args.func(args)


def add_capture_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("run_dir", type=Path, help="compare run directory or *-http directory")
    parser.add_argument(
        "--side",
        choices=["air", "opencode"],
        default=DEFAULT_SIDE,
        help="which captured side to use",
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
                    f"{self.label} reached an edit route at call "
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
    first_write = first_tool_call(cassette, ACTION_TOOLS)
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
        f"- AIR first edit action: `{format_action_call(air_action)}`",
        f"- OpenCode first edit action: `{format_action_call(opencode_action)}`",
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
            if name in ACTION_TOOLS:
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
    if not calls or len(calls) > 3:
        return False
    return all(is_targeted_inspection_call(call) for call in calls)


def is_targeted_inspection_call(call: dict[str, Any]) -> bool:
    name = call.get("name")
    arguments = parse_tool_arguments(call.get("arguments"))
    if not isinstance(arguments, dict):
        return False
    if name == "read":
        return any(arguments.get(key) for key in ("filePath", "path"))
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
    return any(tool in ACTION_TOOLS for tool in tools)


def tool_route_compatible(expected: list[str], actual: list[str]) -> bool:
    if expected == actual:
        return True
    if not expected:
        return not any(tool in ACTION_TOOLS for tool in actual)
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
