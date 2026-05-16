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
bash scripts/verify_code_agent.sh
```

The gate validates the minimal profile, checks the OpenCode-style default tool
surface, and runs a deterministic edit fixture.

## OpenCode Alignment

Use the comparison harness to run the same task through AIR and OpenCode, then
diff the actual OpenAI-compatible HTTP request body:

```bash
python3 scripts/compare_code_agent_io.py run \
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

Each copied workspace is committed to an ephemeral baseline before either agent
runs, so the final diff report only includes files changed by that run even when
the source checkout is dirty.

To analyze existing logs without another model run:

```bash
python3 scripts/compare_code_agent_io.py analyze \
  --air-trace /path/to/air.trace.jsonl \
  --air-http-dir /path/to/air-http \
  --opencode-raw-dir /path/to/opencode-raw \
  --opencode-http-dir /path/to/opencode-http \
  --opencode-events /path/to/opencode.events.jsonl
```

Reports are written under `target/generated/code-agent-io-compare/`.
