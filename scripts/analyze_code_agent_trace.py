#!/usr/bin/env python3
"""Summarize AIR code-agent traces around context sufficiency and write hesitation.

This is intentionally a lightweight offline analyzer. It does not decide whether a
run was correct; it surfaces the model/tool loop shape so dogfood runs can answer:

  1. Did the model receive enough concrete editing context?
  2. Once it had that context, did it choose to write or keep reading?
"""

from __future__ import annotations

import argparse
import json
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any


READ_TOOLS = {"read", "file.read", "file_read"}
SEARCH_TOOLS = {
    "grep",
    "glob",
    "lsp",
}
WRITE_TOOLS = {"edit", "write", "prepare_edit", "commit_edit"}
VERIFY_TOOLS = {"format", "test", "bash"}

PATCH_INTENT_PHRASES = (
    "i have all the context",
    "all the context i need",
    "i have enough",
    "ready to edit",
    "i'll add",
    "i will add",
    "i'll create",
    "i will create",
    "i'll replace",
    "i will replace",
    "i'll refactor",
    "i will refactor",
    "now i need to replace",
    "the change is clear",
)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze AIR code-agent trace model/tool loop behavior."
    )
    parser.add_argument("trace", type=Path, help="AIR trace JSONL file")
    parser.add_argument(
        "--json", action="store_true", help="Emit machine-readable JSON summary"
    )
    parser.add_argument(
        "--max-turns",
        type=int,
        default=80,
        help="Maximum decider turns to print in text mode",
    )
    args = parser.parse_args()

    events = read_events(args.trace)
    summary = analyze_events(events)
    if args.json:
        print(json.dumps(summary, indent=2, ensure_ascii=False))
    else:
        print_text_summary(args.trace, summary, args.max_turns)


def read_events(path: Path) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError as error:
                raise SystemExit(f"{path}:{line_number}: invalid JSONL: {error}") from error
            if isinstance(event, dict):
                events.append(event)
    return events


def analyze_events(events: list[dict[str, Any]]) -> dict[str, Any]:
    tool_events = [
        event
        for event in events
        if event.get("action") == "tool_batch_dispatch_item"
        and event.get("status") in {"ok", "error"}
        and (event.get("meta") or {}).get("tool")
    ]
    tool_counts = Counter((event.get("meta") or {}).get("tool") for event in tool_events)
    tool_group_counts = Counter(tool_group(tool) for tool in tool_counts for _ in range(tool_counts[tool]))
    first_edit = next(
        (
            {
                "step": event.get("step"),
                "rule": event.get("rule"),
                "tool": (event.get("meta") or {}).get("tool"),
            }
            for event in tool_events
            if (event.get("meta") or {}).get("tool") in WRITE_TOOLS
        ),
        None,
    )

    turns = []
    for index, event in enumerate(decider_events(events), start=1):
        turns.append(analyze_decider_turn(index, event))

    final_output = final_return_output(events)
    repeated_reads = repeated_read_counts(tool_events)
    write_hesitation_turns = [
        turn
        for turn in turns
        if turn["context_score"] >= 3
        and not turn["selected_write"]
        and not turn["selected_verify"]
        and turn["selected_tools"]
    ]

    return {
        "events": len(events),
        "decider_calls": len(turns),
        "tool_calls": len(tool_events),
        "tool_counts": dict(sorted(tool_counts.items())),
        "tool_group_counts": dict(sorted(tool_group_counts.items())),
        "first_edit": first_edit,
        "final": summarize_final(final_output),
        "repeated_reads": repeated_reads,
        "write_hesitation_count": len(write_hesitation_turns),
        "write_hesitation_turns": [
            {
                "turn": turn["turn"],
                "step": turn["step"],
                "context_score": turn["context_score"],
                "selected_tools": turn["selected_tools"],
                "patch_intent": turn["patch_intent"],
                "thinking_preview": turn["thinking_preview"],
            }
            for turn in write_hesitation_turns
        ],
        "turns": turns,
    }


def decider_events(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        event
        for event in events
        if event.get("action") == "model_call"
        and (event.get("meta") or {}).get("model") == "code_edit_decider"
    ]


def analyze_decider_turn(index: int, event: dict[str, Any]) -> dict[str, Any]:
    meta = event.get("meta") or {}
    output = event.get("output") if isinstance(event.get("output"), dict) else {}
    selected_tools = [
        tool_call.get("tool")
        for tool_call in output.get("tool_calls") or []
        if isinstance(tool_call, dict) and tool_call.get("tool")
    ]
    selected_repeat_reasons = [
        {
            "tool": tool_call.get("tool"),
            "repeat_reason": repeat_reason_from_input(tool_call.get("input")),
        }
        for tool_call in output.get("tool_calls") or []
        if isinstance(tool_call, dict)
        and tool_call.get("tool")
        and repeat_reason_from_input(tool_call.get("input"))
    ]
    observations = (event.get("input") or {}).get("observations") or []
    evidence = observation_evidence(observations)
    thinking, answer = provider_text(meta.get("provider_response"))
    text = f"{thinking}\n{answer}".lower()
    patch_intent = any(phrase in text for phrase in PATCH_INTENT_PHRASES)
    context_score = evidence["score"] + (1 if patch_intent else 0)

    return {
        "turn": index,
        "step": event.get("step"),
        "input_bytes": meta.get("input_bytes"),
        "provider_request_bytes": meta.get("provider_request_bytes"),
        "provider_user_content_bytes": meta.get("provider_user_content_bytes"),
        "provider_tools_bytes": meta.get("provider_tools_bytes"),
        "complete": output.get("complete"),
        "selected_tools": selected_tools,
        "selected_repeat_reasons": selected_repeat_reasons,
        "selected_write": any(tool in WRITE_TOOLS for tool in selected_tools),
        "selected_verify": any(tool in VERIFY_TOOLS for tool in selected_tools),
        "patch_intent": patch_intent,
        "context_score": context_score,
        "evidence": evidence,
        "thinking_preview": compact_text(thinking, 220),
        "answer_preview": compact_text(answer, 220),
    }


def observation_evidence(observations: Any) -> dict[str, Any]:
    evidence = {
        "read_results": 0,
        "search_results": 0,
        "write_results": 0,
        "verify_results": 0,
        "no_match_results": 0,
        "max_no_match_streak": 0,
        "target_paths": set(),
        "has_code_context": False,
        "has_symbol_or_search": False,
        "has_diagnostics": False,
        "has_prior_edit": False,
        "has_verification": False,
    }
    if not isinstance(observations, list):
        return finish_evidence(evidence)

    for observation in observations:
        if not isinstance(observation, dict):
            continue
        result = observation.get("result")
        if not isinstance(result, list):
            continue
        for item in result:
            if not isinstance(item, dict):
                continue
            tool = item.get("tool")
            output = item.get("output") if isinstance(item.get("output"), dict) else {}
            item_input = item.get("input") if isinstance(item.get("input"), dict) else {}
            path = output.get("path") or item_input.get("path") or item_input.get("filePath")
            if isinstance(path, str) and path:
                evidence["target_paths"].add(path)
            if output.get("no_matches") is True:
                evidence["no_match_results"] += 1
                streak = output.get("no_match_streak")
                if isinstance(streak, int):
                    evidence["max_no_match_streak"] = max(
                        evidence["max_no_match_streak"], streak
                    )
            group = tool_group(tool)
            if group == "read":
                evidence["read_results"] += 1
                if isinstance(output.get("content"), str) and output["content"].strip():
                    evidence["has_code_context"] = True
            elif group == "search":
                evidence["search_results"] += 1
                evidence["has_symbol_or_search"] = True
                for key in ("matches", "symbols", "references", "diagnostics"):
                    values = output.get(key)
                    if isinstance(values, list) and values:
                        evidence["has_symbol_or_search"] = True
                        if key == "diagnostics":
                            evidence["has_diagnostics"] = True
            elif group == "write":
                evidence["write_results"] += 1
                evidence["has_prior_edit"] = item.get("status") == "ok"
            elif group == "verify":
                evidence["verify_results"] += 1
                evidence["has_verification"] = True
                diagnostics = output.get("diagnostics")
                if isinstance(diagnostics, list) and diagnostics:
                    evidence["has_diagnostics"] = True
    return finish_evidence(evidence)


def finish_evidence(evidence: dict[str, Any]) -> dict[str, Any]:
    score = 0
    for key in (
        "has_code_context",
        "has_symbol_or_search",
        "has_diagnostics",
        "has_prior_edit",
        "has_verification",
    ):
        if evidence[key]:
            score += 1
    evidence["target_paths"] = sorted(evidence["target_paths"])
    evidence["score"] = score
    return evidence


def provider_text(response: Any) -> tuple[str, str]:
    if not isinstance(response, dict):
        return "", ""
    choices = response.get("choices")
    if not isinstance(choices, list) or not choices:
        return "", ""
    message = choices[0].get("message") if isinstance(choices[0], dict) else {}
    if not isinstance(message, dict):
        return "", ""
    thinking = message.get("reasoning_content") or message.get("reasoning") or ""
    content = message.get("content") or ""
    return str(thinking), str(content)


def tool_group(tool: Any) -> str:
    if tool in READ_TOOLS:
        return "read"
    if tool in SEARCH_TOOLS:
        return "search"
    if tool in WRITE_TOOLS:
        return "write"
    if tool in VERIFY_TOOLS:
        return "verify"
    return "other"


def repeated_read_counts(tool_events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    counts: Counter[tuple[str, str, str, str]] = Counter()
    reasons: defaultdict[tuple[str, str, str, str], list[str]] = defaultdict(list)
    for event in tool_events:
        tool = (event.get("meta") or {}).get("tool")
        if tool not in READ_TOOLS:
            continue
        payload = event.get("output") if isinstance(event.get("output"), dict) else {}
        item_input = event.get("input") if isinstance(event.get("input"), dict) else {}
        path = payload.get("path") or item_input.get("path") or item_input.get("filePath") or ""
        start = payload.get("start_line") or item_input.get("start_line") or item_input.get("offset") or ""
        end = payload.get("end_line") or item_input.get("end_line") or item_input.get("limit") or ""
        contains = item_input.get("contains") or ""
        key = (tool, str(path), str(start), str(end or contains))
        counts[key] += 1
        reason = repeat_reason_from_input(item_input)
        if reason:
            reasons[key].append(reason)
    repeated = []
    for (tool, path, start, end), count in counts.items():
        if count <= 1:
            continue
        repeated.append(
            {
                "tool": tool,
                "path": path,
                "start_or_offset": start,
                "end_or_limit_or_contains": end,
                "count": count,
                "repeat_reasons": reasons.get((tool, path, start, end), []),
            }
        )
    return sorted(repeated, key=lambda row: row["count"], reverse=True)


def repeat_reason_from_input(input_value: Any) -> str:
    if not isinstance(input_value, dict):
        return ""
    for field in ("repeat_reason", "reread_reason", "search_reason", "reason", "why"):
        value = input_value.get(field)
        if isinstance(value, str) and value.strip():
            return compact_text(value, 180)
    return ""


def final_return_output(events: list[dict[str, Any]]) -> Any:
    for event in reversed(events):
        if event.get("action") == "return":
            return event.get("output")
    return None


def summarize_final(output: Any) -> dict[str, Any] | None:
    if not isinstance(output, dict):
        return None
    edit = output.get("edit") if isinstance(output.get("edit"), dict) else output
    if not isinstance(edit, dict):
        return None
    return {
        "final_success": edit.get("final_success"),
        "patch_applied": edit.get("patch_applied"),
        "changed_files": edit.get("changed_files"),
        "rationale": compact_text(str(edit.get("rationale") or ""), 260),
    }


def compact_text(text: str, limit: int) -> str:
    text = " ".join(text.split())
    if len(text) <= limit:
        return text
    return text[: limit - 3] + "..."


def print_text_summary(path: Path, summary: dict[str, Any], max_turns: int) -> None:
    print(f"Trace: {path}")
    print(
        "Summary: "
        f"events={summary['events']} "
        f"decider_calls={summary['decider_calls']} "
        f"tool_calls={summary['tool_calls']} "
        f"first_edit={summary['first_edit']} "
        f"write_hesitation={summary['write_hesitation_count']}"
    )
    print(f"Tool groups: {summary['tool_group_counts']}")
    print(f"Final: {summary['final']}")
    if summary["repeated_reads"]:
        print("Repeated reads:")
        for row in summary["repeated_reads"][:10]:
            print(f"  - {row}")
    if summary["write_hesitation_turns"]:
        print("Write-hesitation candidates:")
        for row in summary["write_hesitation_turns"][:10]:
            print(
                "  - "
                f"turn={row['turn']} step={row['step']} "
                f"score={row['context_score']} tools={row['selected_tools']} "
                f"intent={row['patch_intent']} thinking={row['thinking_preview']!r}"
            )
    print("")
    print(
        "turn step score tools complete bytes(user/tools/request) evidence thinking"
    )
    for turn in summary["turns"][:max_turns]:
        evidence = turn["evidence"]
        bytes_text = (
            f"{turn['provider_user_content_bytes']}/"
            f"{turn['provider_tools_bytes']}/"
            f"{turn['provider_request_bytes']}"
        )
        evidence_text = (
            f"r{evidence['read_results']} "
            f"s{evidence['search_results']} "
            f"w{evidence['write_results']} "
            f"v{evidence['verify_results']} "
            f"nm{evidence['no_match_results']}/{evidence['max_no_match_streak']}"
        )
        repeat_reason_text = ""
        if turn["selected_repeat_reasons"]:
            repeat_reason_text = " reasons=" + "; ".join(
                f"{item['tool']}:{item['repeat_reason']}"
                for item in turn["selected_repeat_reasons"]
            )
        print(
            f"{turn['turn']:>4} {str(turn['step']):>4} "
            f"{turn['context_score']:>5} "
            f"{','.join(turn['selected_tools']) or '-':<28} "
            f"{str(turn['complete']):<8} "
            f"{bytes_text:<22} "
            f"{evidence_text:<12} "
            f"{turn['thinking_preview']}{repeat_reason_text}"
        )


if __name__ == "__main__":
    main()
