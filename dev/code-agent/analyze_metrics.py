#!/usr/bin/env python3
"""Summarize AIR code-agent benchmark run.json files."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from statistics import mean
from typing import Any


METRIC_KEYS = [
    "model_calls",
    "model_errors",
    "tool_calls",
    "tool_errors",
    "first_edit_tool_index",
    "repeated_reads",
    "verification_tool_calls",
    "verification_failures",
    "repair_iterations",
]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_json", nargs="+", type=Path)
    parser.add_argument("--json", action="store_true", help="print machine-readable JSON")
    args = parser.parse_args()

    reports = [load_report(path) for path in args.run_json]
    summary = summarize_reports(reports)
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
        return
    print_text_summary(summary)


def load_report(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        report = json.load(handle)
    report["_path"] = str(path)
    return report


def summarize_reports(reports: list[dict[str, Any]]) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []
    for report in reports:
        for task in report.get("tasks", []):
            metrics = task.get("metrics", {})
            row = {
                "run_json": report.get("_path"),
                "suite": report.get("suite"),
                "task": task.get("id"),
                "mode": task.get("mode"),
                "pass": bool(task.get("pass")),
                "model_calls_spent": task.get("model_calls_spent"),
                "changed_files": task.get("changed_files", []),
                "trace_path": task.get("trace_path"),
                "output_path": task.get("output_path"),
                "error": task.get("error"),
            }
            for key in METRIC_KEYS:
                row[key] = metrics.get(key)
            rows.append(row)

    numeric: dict[str, dict[str, float | int | None]] = {}
    for key in METRIC_KEYS:
        values = [row[key] for row in rows if isinstance(row.get(key), (int, float))]
        numeric[key] = {
            "avg": mean(values) if values else None,
            "min": min(values) if values else None,
            "max": max(values) if values else None,
        }

    total = len(rows)
    passed = sum(1 for row in rows if row["pass"])
    return {
        "total": total,
        "passed": passed,
        "failed": total - passed,
        "pass_rate": passed / total if total else None,
        "metrics": numeric,
        "tasks": rows,
    }


def print_text_summary(summary: dict[str, Any]) -> None:
    print(
        f"tasks={summary['total']} passed={summary['passed']} "
        f"failed={summary['failed']} pass_rate={format_rate(summary['pass_rate'])}"
    )
    for key in METRIC_KEYS:
        stats = summary["metrics"][key]
        print(
            f"{key}: avg={format_number(stats['avg'])} "
            f"min={format_number(stats['min'])} max={format_number(stats['max'])}"
        )
    print()
    for task in summary["tasks"]:
        status = "PASS" if task["pass"] else "FAIL"
        changed = ", ".join(task.get("changed_files") or [])
        print(
            f"{status} {task['task']} ({task.get('mode') or 'unknown'}): "
            f"spent_model={task.get('model_calls_spent')} "
            f"trace_model={task.get('model_calls')} tool={task.get('tool_calls')} "
            f"first_edit={task.get('first_edit_tool_index')} "
            f"repeated_reads={task.get('repeated_reads')} changed=[{changed}]"
        )
        if task.get("error"):
            print(f"  error: {task['error']}")


def format_number(value: Any) -> str:
    if value is None:
        return "-"
    if isinstance(value, float):
        return f"{value:.2f}"
    return str(value)


def format_rate(value: Any) -> str:
    if value is None:
        return "-"
    return f"{value:.1%}"


if __name__ == "__main__":
    main()
