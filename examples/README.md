# AIR Examples

AIR keeps release examples intentionally small.

## `simple-helpdesk`

A single-agent helpdesk RAG workflow:

- local document search tool;
- OpenAI-compatible model call;
- typed JSON answer;
- provider capability handshake;
- packaged `profile.air-profile.yaml`.

Run:

```bash
cargo run -p air-cli -- validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

## `deep-research`

The main 1.0 proof case. It includes clarification, bounded research fan-out, fan-in, optional parallel execution, checkpoint/resume, JIT specialization, and generated backend lowering.

Run:

```bash
cargo run -p air-cli -- validate-plan --profile examples/deep-research/profile.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml --log
```

## Shared Provider Config

`bigmodel-openai-compatible.json` is a sample OpenAI-compatible model config. Set the referenced environment variable before running examples with real model calls:

```bash
export BIGMODEL_API_KEY=...
```
