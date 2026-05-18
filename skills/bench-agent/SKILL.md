# AIR Bench Agent

Use this executor for controlled comparisons between AIR agent/tool/profile variants, such as Semble versus grep/read/LSP, core code-agent versus subagent-enabled code-agent, or one prompt/profile shape versus another.

## Operating Model

Run experiments in isolated temporary workdirs, usually under `/tmp/air-bench-agent-*`, and leave the source workspace unchanged. Prefer using existing AIR benchmark commands and trace artifacts. When a candidate needs temporary profiles or tool configs, create them inside the temporary clone/workdir and include their paths in the report.

## Benchmark Method

For each comparison:

1. Define the task and the compared variants.
2. Run a baseline first.
3. Run each candidate with the same task, model config, and comparable budget.
4. Capture trace paths and output/artifact paths.
5. Extract metrics from trace JSONL:
   - `model_calls`
   - `tool_calls`
   - per-tool counts
   - `max_provider_request_bytes`
   - `sum_provider_request_bytes`
   - `max_input_bytes`
   - `sum_input_bytes`
   - child subagent trace metrics when present
6. Check whether each run completed the task correctly and whether verification passed.
7. Explain the first meaningful behavioral divergence.
8. Recommend whether the candidate should be default, optional, rejected, or tested further.

## Report Format

Return a concise structured report:

```text
Task
- ...

Variants
- baseline: ...
- candidate: ...

Metrics
| variant | success | model calls | tool calls | request bytes | notes |

Trace Findings
- ...

Recommendation
- default / optional / reject / needs more benchmark
- rationale
```

## Decision Rules

A candidate is not worth default integration unless it improves success rate or materially reduces model calls/request bytes without increasing failure modes. A candidate may still be worth optional integration if it helps a narrow class of tasks and stays isolated from the default code-agent path.
