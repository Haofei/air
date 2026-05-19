# AIR Examples

AIR keeps examples intentionally small.

## `simple-helpdesk`

A single-agent helpdesk RAG workflow:

- local document search tool;
- OpenAI-compatible model call;
- typed JSON answer;
- provider capability handshake;
- packaged `profile.air-profile.yaml`.

Run:

```bash
cargo run -p air-cli -- dev validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
cargo run -p air-cli -- dev run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

## `deep-research`

The main deep-research proof case. It includes clarification, bounded research fan-out, fan-in, optional parallel execution, checkpoint/resume, and JIT specialization.

Run:

```bash
cargo run -p air-cli -- dev validate-plan --profile examples/deep-research/profile.air-profile.yaml
cargo run -p air-cli -- dev run-plan --profile examples/deep-research/profile.air-profile.yaml --log
```

## `skills/code-agent`

A bounded coding-agent skill proof case. It exposes one OpenCode-style edit loop:
the model chooses bounded search/context/edit/verify tools under AIR policy and
trace capture. Deterministic fixtures and alternate tool configs live under
`skills/code-agent/fixtures/`.

Run:

```bash
cargo run -p air-cli -- skill validate code-agent
cargo run -p air-cli -- skill explain code-agent
```

## `context-compact`

A reusable context-budget gate for any agent domain. The implementation lives in
`modules/std/context/compact.air.yaml`, so coding, research, support, and planning agents can all
compose the same standard module. It measures an arbitrary JSON payload with `context.measure`,
skips the model call when the payload is under budget, and calls the shared `context_compactor`
model only when semantic compaction is needed.

Run the deterministic under-budget path without a model config:

```bash
cargo run -p air-cli -- dev validate-plan --profile examples/context-compact/profile.air-profile.yaml
cargo run -p air-cli -- dev run-plan examples/context-compact/context-compact.air-plan.yaml \
  --store modules/std/module-store.air-store.yaml \
  --input examples/context-compact/input.json \
  --tool-config examples/context-compact/tools.json \
  --log
```

## Shared Provider Config

`local-openai-compatible.json` is the default OpenAI-compatible model config. `air` auto-loads a repository-root `.env` before real model calls:

```dotenv
AIR_MODEL_LOCAL_API_KEY=...
AIR_MODEL_LOCAL_BASE_URL=http://localhost:11434/v1
AIR_MODEL_LOCAL_MODEL=qwen2.5-coder
```

Use `--model-config examples/bigmodel-openai-compatible.json` when you want the
older environment-profile switching behavior.
