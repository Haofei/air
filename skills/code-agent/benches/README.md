# Code-Agent Benchmarks

This directory contains deterministic coding tasks for measuring AIR code-agent
changes. The benchmark runner uses the real AIR code-agent loop, an isolated
fixture workspace, normal verification commands, raw AIR traces, and diff
constraints.

Run one task:

```bash
cargo run -p air-cli -- bench code --limit 1 --keep-workdirs
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
cargo run -p air-cli -- bench code --refresh
```

Run the non-Rust Node suite:

```bash
cargo run -p air-cli -- bench code \
  --suite skills/code-agent/benches/node-small/suite.json \
  --refresh
```

Run the subagent smoke suite:

```bash
cargo run -p air-cli -- bench code \
  --suite skills/code-agent/benches/rust-subagent-smoke/suite.json \
  --refresh \
  --keep-workdirs
```

The runner writes `run.json`, per-task `trace.jsonl`, per-task `output.json`,
an `artifact/` copy, and failed task workdirs under
`target/generated/code-agent-bench/<run-id>/`.

Generate a Markdown report while running the suite:

```bash
cargo run -p air-cli -- bench code \
  --suite skills/code-agent/benches/rust-small/suite.json \
  --report target/generated/code-agent-bench/rust-small.md
```

For skill A/B checks, compare against a no-skill baseline in the same report:

```bash
cargo run -p air-cli -- bench skill tdd-workflow \
  --suite skills/code-agent/benches/rust-small/suite.json \
  --compare-no-skill \
  --report target/generated/code-agent-bench/tdd-workflow.md
```

The first baseline suite is `rust-small`: a set of classic Rust refactor
tasks that should preserve behavior, pass `cargo test -q`, and touch only the
requested source file. `node-small` provides the same shape for JavaScript using
Node's built-in test runner.

`rust-subagent-smoke` enables the `task` subagent tool and writes nested child
trace/output paths into the parent trace. Use it to compare whether isolated
read-only exploration reduces parent model/tool calls without turning subagents
into an opaque cost sink.
