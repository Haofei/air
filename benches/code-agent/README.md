# Code-Agent Benchmarks

This directory contains deterministic coding tasks for measuring AIR code-agent
changes. The benchmark runner uses the real AIR code-agent loop, an isolated
fixture workspace, normal verification commands, raw AIR traces, and diff
constraints.

Run one task:

```bash
cargo run -p air-cli -- bench code-agent --limit 1 --keep-workdirs
```

By default, the runner reuses code-run artifacts from
`target/generated/code-run-artifacts/` when the task, profile, model config,
tool config, fixture snapshot, and constraints match a previous run. Replayed
runs apply the cached diff and spend zero model calls. Use `--refresh` when you
want to force a live model run and replace the cache entry.

Use replayed runs for debugging artifact/replay behavior and report generation.
Use `--refresh` for stability measurements: replay does not exercise the current
model, runtime, or tool implementation.

Run the full small Rust suite:

```bash
cargo run -p air-cli -- bench code-agent --refresh
```

The runner writes `run.json`, per-task `trace.jsonl`, per-task `output.json`,
an `artifact/` copy, and failed task workdirs under
`target/generated/code-agent-bench/<run-id>/`.

Summarize one or more runs:

```bash
python3 dev/code-agent/analyze_metrics.py \
  target/generated/code-agent-bench/<run-id>/run.json
```

The first baseline suite is `rust-small`: ten small Rust refactor tasks that
should preserve behavior, pass `cargo test -q`, and touch only the requested
source file.
