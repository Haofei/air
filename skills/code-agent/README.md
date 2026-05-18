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

`code-edit-loop.air.yaml` is wired by `code-edit.air-plan.yaml` and
`edit.air-profile.yaml`.

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
- `write`
- `apply_patch`
- `task`
- `webfetch`
- `todowrite`
- `todoread`
- `skill`

The model runs verification and git inspection through `bash`, matching the
OpenCode-style terminal workflow instead of exposing AIR-specific git wrappers.
If verification fails, the result goes back into the next model turn as ordinary
tool output.

`task` launches a separate non-mutating AIR exploration loop through
`explore.air-profile.yaml`. Use it for broad investigation that would otherwise
fill the main edit loop context. Each subagent call writes its own child
trace/output under `.air/subagents/` and returns those paths in the parent trace
so the handoff stays concise without losing auditability.

`tools.json` uses the same OpenCode-style tool names against the current repository.

## Dogfood

```bash
cargo run -p air-cli -- run "refactor a small helper and run tests" \
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config skills/code-agent/tools.json \
  --trace-out target/generated/code-agent.trace.jsonl \
  --log
```

## Verification

```bash
cargo test --workspace code_agent
```

The workspace tests validate the minimal profile, check the OpenCode-style
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
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config skills/code-agent/tools.json \
  --artifact-out target/generated/code-run-artifacts/manual-run
```

The artifact contains the input fingerprint, workspace snapshots, final output,
trace, diff, and structured failure reason. The benchmark runner uses the same
artifact format as its replay cache, but the format is not benchmark-specific.

## OpenCode Alignment

Use the comparison harness to run the same task through AIR and OpenCode, then
diff the actual OpenAI-compatible HTTP request body:

```bash
python3 dev/code-agent/compare.py run --name todo-refactor
```

By default the harness uses the modified local OpenCode checkout at
`/Users/hwang/work/opencode` when it exists. Override it with
`--opencode-command` only when comparing against another OpenCode build. The
OpenCode command must support `OPENCODE_RAW_IO_DIR`; the harness fails if no
`*provider-request.json` files are produced.

When `OPENAI_BASE_URL` is set, the harness starts local forwarding proxies for
both AIR and OpenCode, points each client at its proxy, and gives both clients
the same `OPENAI_API_KEY`, `OPENAI_BASE_URL`, and `OPENAI_MODEL`. It records the
final POST JSON body under `air-http/` and `opencode-http/`, then forwards the
request to the real provider. Authorization headers are redacted; request bodies
are kept intact for shape comparison.

AIR runs in the current repository by default, while OpenCode always runs in an
isolated copy for comparison. The harness snapshots AIR before it runs and commits
OpenCode's copy to an ephemeral baseline, so the final diff report only includes
files changed by that run even when
the source checkout is dirty.

To analyze existing logs without another model run:

```bash
python3 dev/code-agent/compare.py analyze \
  --air-trace /path/to/air.trace.jsonl \
  --air-http-dir /path/to/air-http \
  --opencode-raw-dir /path/to/opencode-raw \
  --opencode-http-dir /path/to/opencode-http \
  --opencode-events /path/to/opencode.events.jsonl
```

Reports are written under `target/generated/code-agent-io-compare/`.

When HTTP capture is enabled, the harness runs OpenCode first and uses its
model-call route as a reference while AIR runs. If AIR's next tool choice clearly
diverges from the same OpenCode call, the harness records the divergence and
stops the next AIR model request locally instead of spending the rest of the
budget on a known-bad path.

## Relay Debugging

When a run diverges late, prefer AIR artifact replay. `compare.py replay-air`
uses the saved `air-artifact/` directory from a previous comparison. Without
`--from`, it applies the cached AIR diff without model calls. With `--from N`,
AIR replays model outputs before 1-based trace event line `N`, then continues
with the live provider.

Inspect a previous run:

```bash
python3 dev/code-agent/relay.py inspect \
  target/generated/code-agent-io-compare/todo-refactor \
  --side air
```

Compare AIR and OpenCode at a specific model call:

```bash
python3 dev/code-agent/relay.py diff \
  target/generated/code-agent-io-compare/todo-refactor \
  --call 9
```

Find the first likely divergence and suggested replay points:

```bash
python3 dev/code-agent/relay.py divergence \
  target/generated/code-agent-io-compare/todo-refactor
```

Replay from trace event 23:

```bash
python3 dev/code-agent/compare.py replay-air \
  target/generated/code-agent-io-compare/todo-refactor \
  --from 23 \
  --name todo-refactor-replay
```

`relay.py` remains useful for inspecting older raw HTTP captures and comparing
request/response shape at a specific model call, but AIR replay no longer
depends on the Python relay.
