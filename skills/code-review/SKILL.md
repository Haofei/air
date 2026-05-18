# Code Review

Use this skill when reviewing a pull request, merge request, diff, patch, or changed files.

## Objective

Find concrete risks in the proposed change. Prioritize defects that could cause incorrect behavior, production incidents, security issues, data loss, broken compatibility, missing verification, or maintainability problems that would block safe merge.

## Review Method

1. Establish the change intent from the title, description, task, branch names, and touched files.
2. Inspect the diff, not just the surrounding code.
3. For each risky change, reason from current behavior to new behavior and identify the failing scenario.
4. Check whether tests cover the changed behavior and edge cases.
5. Prefer source-grounded findings over broad advice.

## Output Format

Lead with findings. If there are no actionable findings, say that clearly.

For each finding include:

- Severity: `P0`, `P1`, `P2`, or `P3`
- Location: file and line when available
- Problem: what is wrong
- Impact: what can fail or regress
- Suggested fix: concise, practical remediation

After findings, include:

- Open questions or assumptions
- Test/verification gaps
- Brief summary of the change

## Severity Guide

- `P0`: must fix immediately; security breach, data loss, major outage, or unusable release
- `P1`: high-confidence correctness/security issue that should block merge
- `P2`: real bug or missing verification that is worth fixing before merge
- `P3`: low-risk maintainability, clarity, or follow-up issue

## Review Rules

- Do not nitpick style unless it can hide a bug or meaningfully harm maintainability.
- Do not propose unrelated refactors.
- Do not claim a bug without a plausible failing path.
- Do not post comments to remote systems unless explicitly asked.
- If the diff is incomplete, state the limits of the review and review what is available.
- If line numbers are unavailable, reference the file/function/changed block.
