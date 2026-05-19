# AIR Code Agent Skill

This example is intentionally small. It is a minimal coding loop that lets the
model use familiar tools, while AIR records the trace, enforces capabilities,
runs verification, and reports the final diff.

## Interface

```bash
cargo run -p air-cli -- run "fix the failing add function and retest"
```

There is one built-in skill, one loop, and one default tool set. The default path
is a direct search/context/edit/verify loop.

The skill manifest is `air-skill.yaml`. Use these commands to inspect it:

```bash
cargo run -p air-cli -- skill validate code-agent
cargo run -p air-cli -- skill explain code-agent
cargo run -p air-cli -- skill audit code-agent
```

## Loop

The default `edit.air-profile.yaml` selects the native Rust `code-edit` loop.
There is no YAML state-machine executor in the product path.

```text
init -> choose -> act -> choose -> ... -> summarize -> done
```

The model sees the user task, recent tool results, and these tools:

- `question`
- `bash`
- `glob`
- `grep`
- `read_contains`
- `read_range`
- `edit`
- `task`
- `webfetch`
- `todowrite`
- `todoread`
- `skill`

The model runs verification and git inspection through `bash`, matching the
OpenCode-style terminal workflow instead of exposing AIR-specific git wrappers.
If verification fails, the result goes back into the next model turn as ordinary
tool output.

`task` launches a separate non-mutating native exploration loop through
`air dev native-loop --kind explore`. Use it for broad investigation that would
otherwise fill the main edit loop context. Each subagent call writes its own
child trace/output under `.air/subagents/` and returns those paths in the parent
trace so the handoff stays concise without losing auditability.

`tools.json` uses the same OpenCode-style tool names against the current repository.

## Dogfood

```bash
cargo run -p air-cli -- run "refactor a small helper and run tests" \
  --model-config examples/local-openai-compatible.json \
  --tool-config skills/code-agent/tools.json \
  --trace-out target/generated/code-agent.trace.jsonl \
  --log
```

## Verification

```bash
cargo test --workspace code_agent
```

The workspace tests validate the native profile, check the OpenCode-style
default tool surface, and run a deterministic edit fixture.

The skill also carries its own benchmark suites under `benches/`:

```bash
cargo run -p air-cli -- bench code --suite skills/code-agent/benches/rust-small/suite.json --limit 1
```

## Code-Run Artifacts

Use `--artifact-out` when you want an auditable, replayable record of one code
agent run:

```bash
cargo run -p air-cli -- run "refactor a small helper and run tests" \
  --model-config examples/local-openai-compatible.json \
  --tool-config skills/code-agent/tools.json \
  --artifact-out target/generated/code-run-artifacts/manual-run
```

The artifact contains the input fingerprint, workspace snapshots, final output,
trace, diff, and structured failure reason. The benchmark runner uses the same
artifact format as its replay cache, but the format is not benchmark-specific.

## Benchmark And Trace Investigation

Use the AIR-native benchmark runner for repeatable code-agent measurements:

```bash
cargo run -p air-cli -- bench code \
  --suite skills/code-agent/benches/rust-small/suite.json \
  --report target/generated/code-agent-bench/rust-small.md
```

Use skill benchmarks when measuring whether an instruction skill improves a
task class:

```bash
cargo run -p air-cli -- bench skill tdd-workflow \
  --suite skills/code-agent/benches/rust-small/suite.json \
  --compare-no-skill \
  --report target/generated/code-agent-bench/tdd-workflow.md
```

For tool/profile experiments, use the bench executor through the public entry
agent instead of local Python harnesses:

```bash
cargo run -p air-cli -- run \
  "compare grep, LSP, and semantic search on a medium refactor; report request size and model/tool calls" \
  --mode bench \
  --log
```

`air bench` writes `run.json`, traces, artifacts, and optional Markdown reports.
`air run --mode bench` is for exploratory benchmark design and produces a
normal AIR trace/artifact instead of an external debug format.
