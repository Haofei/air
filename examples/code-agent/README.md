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

- `glob`
- `read`
- `grep`
- `lsp`
- `edit`
- `bash`
- `todowrite`

The model runs verification and git inspection through `bash`, matching the
OpenCode-style terminal workflow instead of exposing AIR-specific git wrappers.
If verification fails, the result goes back into the next model turn as ordinary
tool output.

`tools.dogfood.json` uses the same seven tools against the current repository.

## Dogfood

```bash
cargo run -p air-cli -- code "refactor a small helper and run tests" \
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config examples/code-agent/tools.dogfood.json \
  --trace-out target/generated/code-agent.trace.jsonl \
  --log
```

## Verification

```bash
bash scripts/verify_code_agent.sh
```

The gate validates the minimal profile, checks that only the seven default tools
are exposed, and runs a deterministic edit fixture.
