# AIR Code Agent

This example is intentionally small. It is a minimal coding loop that lets the
model use familiar tools, while AIR records the trace, enforces capabilities,
runs verification, and reports the final diff.

## Interface

```bash
cargo run -p air-cli -- code "fix the failing add function and retest"
```

There is one loop and one default tool set. The default path is a direct
read/search/edit/verify loop.

## Loop

`code-edit-loop.air.yaml` is wired by `code-edit.air-plan.yaml` and
`edit.air-profile.yaml`.

```text
init -> choose -> act -> choose -> ... -> summarize -> done
```

The model sees the user task, recent tool results, and these tools:

- `question`
- `bash`
- `read`
- `glob`
- `grep`
- `edit`
- `write`
- `task`
- `webfetch`
- `todowrite`
- `todoread`
- `skill`

The model runs verification and git inspection through `bash`, matching the
OpenCode-style terminal workflow instead of exposing AIR-specific git wrappers.
If verification fails, the result goes back into the next model turn as ordinary
tool output.

`tools.json` uses the same OpenCode-style tool names against the current repository.

## Dogfood

```bash
cargo run -p air-cli -- code "refactor a small helper and run tests" \
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config examples/code-agent/tools.json \
  --trace-out target/generated/code-agent.trace.jsonl \
  --log
```

## Verification

```bash
cargo test --workspace code_agent
```

The workspace tests validate the minimal profile, check the OpenCode-style
default tool surface, and run a deterministic edit fixture.

## OpenCode Alignment

Use the comparison harness to run the same task through AIR and OpenCode, then
diff the actual OpenAI-compatible HTTP request body:

```bash
python3 dev/code-agent/compare.py run \
  --name todo-refactor \
  --opencode-model zhipuai-coding-plan/glm-5.1
```

By default the harness uses the modified local OpenCode checkout at
`/Users/hwang/work/opencode` when it exists. Override it with
`--opencode-command` only when comparing against another OpenCode build. The
OpenCode command must support `OPENCODE_RAW_IO_DIR`; the harness fails if no
`*provider-request.json` files are produced.

When `OPENAI_BASE_URL` is set, the harness starts local forwarding proxies for
both AIR and OpenCode, points each client at its proxy, records the final POST
JSON body under `air-http/` and `opencode-http/`, then forwards the request to
the real provider. Authorization headers are redacted; request bodies are kept
intact for shape comparison.

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

When a run diverges late, use the relay to replay the known-good prefix and
continue from a specific model call instead of spending the whole budget again.

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

Resume from call 23:

```bash
python3 dev/code-agent/relay.py run \
  target/generated/code-agent-io-compare/todo-refactor \
  --side air \
  --from 23 \
  -- cargo run -p air-cli -- code "refactor a small helper and run tests" \
    --model-config examples/bigmodel-openai-compatible.json \
    --tool-config examples/code-agent/tools.json \
    --trace-out target/generated/code-agent.trace.jsonl \
    --log
```

Calls before `--from` are served from the captured HTTP responses. Calls at and
after `--from` are forwarded to the original provider URL inferred from the
capture. The relay writes a new `relay-air-http/` directory so request shape and
response behavior can be compared at the fork point.

To keep the normal comparison report flow and reuse a saved OpenCode run:

```bash
python3 dev/code-agent/compare.py replay-air \
  target/generated/code-agent-io-compare/todo-refactor \
  --from 23 \
  --name todo-refactor-replay
```
