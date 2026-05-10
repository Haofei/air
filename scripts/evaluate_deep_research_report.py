#!/usr/bin/env python3
"""Evaluate the minimum quality contract for an AIR deep-research report."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Check that a deep-research output is more than a short summary."
    )
    parser.add_argument("path", type=Path, help="Path to AIR output JSON")
    parser.add_argument("--min-report-words", type=int, default=900)
    parser.add_argument("--min-key-findings", type=int, default=4)
    parser.add_argument("--min-comparison-rows", type=int, default=4)
    parser.add_argument("--min-recommendations", type=int, default=3)
    parser.add_argument("--min-sources", type=int, default=4)
    parser.add_argument("--min-limitations", type=int, default=2)
    return parser.parse_args()


def report_object(output: Any) -> dict[str, Any]:
    if not isinstance(output, dict):
        raise ValueError("output JSON must be an object")
    for key in ("final_report", "result", "report"):
        candidate = output.get(key)
        if isinstance(candidate, dict) and isinstance(candidate.get("report"), str):
            return candidate
    if isinstance(output.get("report"), str):
        return output
    raise ValueError("could not find report object at final_report, result, or report")


def word_count(text: str) -> int:
    return len(re.findall(r"[A-Za-z0-9][A-Za-z0-9'_-]*", text))


def require(condition: bool, message: str, errors: list[str]) -> None:
    if not condition:
        errors.append(message)


def is_non_empty_string(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


def is_string_list(value: Any, min_items: int = 1) -> bool:
    return (
        isinstance(value, list)
        and len(value) >= min_items
        and all(is_non_empty_string(item) for item in value)
    )


def evaluate(report: dict[str, Any], args: argparse.Namespace) -> list[str]:
    errors: list[str] = []
    body = report.get("report", "")
    require(is_non_empty_string(report.get("title")), "missing title", errors)
    require(
        word_count(str(report.get("executive_summary", ""))) >= 40,
        "executive_summary must be at least 40 words",
        errors,
    )
    require(
        word_count(body) >= args.min_report_words,
        f"report must be at least {args.min_report_words} words",
        errors,
    )
    require(
        is_string_list(report.get("key_findings"), args.min_key_findings),
        f"key_findings must have at least {args.min_key_findings} non-empty items",
        errors,
    )

    matrix = report.get("comparison_matrix")
    require(
        isinstance(matrix, list) and len(matrix) >= args.min_comparison_rows,
        f"comparison_matrix must have at least {args.min_comparison_rows} rows",
        errors,
    )
    if isinstance(matrix, list):
        for index, row in enumerate(matrix):
            require(isinstance(row, dict), f"comparison_matrix[{index}] must be an object", errors)
            if isinstance(row, dict):
                for field in ("dimension", "assessment", "evidence", "confidence"):
                    require(
                        is_non_empty_string(row.get(field)),
                        f"comparison_matrix[{index}].{field} must be non-empty",
                        errors,
                    )
                require(
                    row.get("confidence") in ("low", "medium", "high"),
                    f"comparison_matrix[{index}].confidence must be low, medium, or high",
                    errors,
                )

    recommendations = report.get("recommendations")
    require(
        isinstance(recommendations, list) and len(recommendations) >= args.min_recommendations,
        f"recommendations must have at least {args.min_recommendations} items",
        errors,
    )
    if isinstance(recommendations, list):
        for index, row in enumerate(recommendations):
            require(isinstance(row, dict), f"recommendations[{index}] must be an object", errors)
            if isinstance(row, dict):
                for field in ("action", "rationale", "priority"):
                    require(
                        is_non_empty_string(row.get(field)),
                        f"recommendations[{index}].{field} must be non-empty",
                        errors,
                    )
                require(
                    row.get("priority") in ("low", "medium", "high"),
                    f"recommendations[{index}].priority must be low, medium, or high",
                    errors,
                )

    require(
        is_string_list(report.get("sources"), args.min_sources),
        f"sources must have at least {args.min_sources} non-empty items",
        errors,
    )
    require(
        is_string_list(report.get("limitations"), args.min_limitations),
        f"limitations must have at least {args.min_limitations} non-empty items",
        errors,
    )
    require(
        is_string_list(report.get("open_questions"), 1),
        "open_questions must have at least one non-empty item",
        errors,
    )
    return errors


def main() -> int:
    args = parse_args()
    try:
        output = json.loads(args.path.read_text(encoding="utf-8"))
        report = report_object(output)
        errors = evaluate(report, args)
    except Exception as error:  # noqa: BLE001 - CLI should report validation failures.
        print(f"[air-report-eval] {args.path}: error: {error}", file=sys.stderr)
        return 1

    if errors:
        print(f"[air-report-eval] {args.path}: failed", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print(
        json.dumps(
            {
                "path": str(args.path),
                "report_words": word_count(report["report"]),
                "key_findings": len(report.get("key_findings", [])),
                "comparison_rows": len(report.get("comparison_matrix", [])),
                "recommendations": len(report.get("recommendations", [])),
                "sources": len(report.get("sources", [])),
                "limitations": len(report.get("limitations", [])),
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
