# Code Agent Dev Tools

This directory contains local development tools for comparing AIR's code-agent loop with OpenCode and debugging request-level divergences.

## Compare

Run the same task through OpenCode and AIR, capture provider HTTP request/response JSON, and write a comparison report:

```bash
python3 dev/code-agent/compare.py run \
  --name todo-refactor \
  --opencode-model zhipuai-coding-plan/glm-5.1
```

The harness runs OpenCode first, then AIR. When a previous OpenCode capture is available, use `replay-air` to reuse it and avoid spending model calls on both systems again:

```bash
python3 dev/code-agent/compare.py replay-air \
  target/generated/code-agent-io-compare/todo-refactor \
  --from 23 \
  --name todo-refactor-replay
```

## Relay

Inspect a previous run or replay captured AIR/OpenCode HTTP calls from a specific model-call index:

```bash
python3 dev/code-agent/relay.py inspect \
  target/generated/code-agent-io-compare/todo-refactor \
  --side air

python3 dev/code-agent/relay.py diff \
  target/generated/code-agent-io-compare/todo-refactor \
  --call 9
```

These tools are intentionally not part of the AIR CLI. They are dogfood/debug infrastructure for keeping AIR's code-agent request shape close to OpenCode.
