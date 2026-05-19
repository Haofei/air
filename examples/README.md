# AIR Examples

AIR keeps examples intentionally small and product-shaped. Legacy YAML
state-machine demos were removed as AIR moved to native Rust loops.

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

## Shared Provider Config

`local-openai-compatible.json` is the default OpenAI-compatible model config. `air` auto-loads a repository-root `.env` before real model calls:

```dotenv
AIR_MODEL_LOCAL_API_KEY=...
AIR_MODEL_LOCAL_BASE_URL=http://localhost:11434/v1
AIR_MODEL_LOCAL_MODEL=qwen2.5-coder
```

Use `--model-config examples/bigmodel-openai-compatible.json` when you want the
older environment-profile switching behavior.
