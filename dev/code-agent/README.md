# Code Agent Dev Tools

This directory contains local development tools for comparing AIR's code-agent loop with OpenCode and debugging request-level divergences.

## Compare

Run the same task through OpenCode and AIR, capture provider HTTP request/response JSON, and write a comparison report:

```bash
python3 dev/code-agent/compare.py run \
  --name todo-refactor
```

The harness reads the repository `.env` for AIR provider settings. Use
`AIR_MODEL_PROFILE=glm` or `AIR_MODEL_PROFILE=local` to switch the comparison.
AIR and OpenCode both use the selected `OPENAI_API_KEY`, `OPENAI_BASE_URL`, and
`OPENAI_MODEL`; OpenCode receives a temporary compare-only config for that run.

The harness runs OpenCode first, then AIR. Every AIR run writes an
`air-artifact/` directory. Use `replay-air` to reuse that artifact without
spending model calls again, or pass `--from N` to replay the prefix before
1-based AIR trace event line `N` and continue with the live provider:

```bash
python3 dev/code-agent/compare.py replay-air \
  target/generated/code-agent-io-compare/todo-refactor \
  --from 23 \
  --name todo-refactor-replay
```

## Relay

Inspect a previous run or compare captured AIR/OpenCode HTTP calls:

```bash
python3 dev/code-agent/relay.py inspect \
  target/generated/code-agent-io-compare/todo-refactor \
  --side air

python3 dev/code-agent/relay.py diff \
  target/generated/code-agent-io-compare/todo-refactor \
  --call 9
```

`relay.py` is now mostly an inspection tool for old HTTP captures. New AIR
replay should go through `compare.py replay-air`, which delegates replay to AIR
code-run artifacts instead of emulating AIR state in Python.

## Benchmark Metrics

Use the AIR-native benchmark runner when you want a repeatable baseline instead
of an OpenCode comparison:

```bash
cargo run -p air-cli -- bench code-agent --limit 1 --keep-workdirs
python3 dev/code-agent/analyze_metrics.py \
  target/generated/code-agent-bench/<run-id>/run.json
```

The benchmark runner records pass/fail, changed files, model calls, tool calls,
repeated reads, verification failures, and the first edit tool index for each
task.

Benchmark runs use AIR code-run artifacts as their cache layer. A fresh run
with `--refresh` writes an artifact containing the task fingerprint, workspace
snapshot, trace, output, diff, and structured failure reason. Later runs with
the same fingerprint replay that artifact locally and report
`model_calls_spent: 0`, so you can iterate on metrics and reporting without
paying for another model run.

Treat replayed benchmark runs as cache/replay checks, not stability
measurements. Use `--refresh` when comparing code-agent behavior across AIR
runtime or tool changes.

The same artifact path can be used outside benchmarks:

```bash
cargo run -p air-cli -- skill run code-agent "refactor the target helper" \
  --artifact-out target/generated/code-run-artifacts/manual-run
```
