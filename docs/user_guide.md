# AIR User Guide

This guide is the shortest path from a clean checkout to running and modifying AIR workflows.

AIR has three layers:

- **Module**: a typed, bounded state machine in `.air.yaml`.
- **RunPlan**: a verified DAG that connects modules, including dynamic fan-out/fan-in.
- **Runtime/backend**: native AIR VM, LangGraph, or OpenAI JS strict generated runtime.

The native VM is the conformance runtime. Generated backends preserve AIR's checked topology and host contract, while host applications still provide model, tool, and approval implementations.

## 1. Run The Simple Example

```bash
cargo run -p air-cli -- validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml

export BIGMODEL_API_KEY=...
export BIGMODEL_BASE_URL=https://open.bigmodel.cn/api/coding/paas/v4
export BIGMODEL_MODEL=GLM-5.1
cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

The profile points to:

- `examples/simple-helpdesk/helpdesk-rag.air-plan.yaml`
- `examples/simple-helpdesk/module-store.air-store.yaml`
- `examples/simple-helpdesk/input.json`
- `examples/simple-helpdesk/tools.json`
- `examples/bigmodel-openai-compatible.json`

Profiles are the preferred user-facing entrypoint because they hide repeated flags.

## 2. Module Anatomy

A module is a bounded state machine:

```yaml
agent:
  name: helpdesk-rag-agent
  version: 0.1.0

inputs:
  question: string

outputs:
  answer:
    type: object
    required: [answer, citations, escalation_required]

requires:
  capabilities: [retrieval.local]

state:
  phase:
    type: enum
    enum: [init, retrieve, answer, done, failed]

tools:
  - name: docs.search
    capability: retrieval.local

workflow:
  kind: state_machine
  initial: init
  max_steps: 8
  terminal: [done, failed]
  rules:
    - id: retrieve
      when: phase == "retrieve"
      actions:
        - kind: tool_call
          tool: docs.search
          input: { object: { query: { ref: question } } }
          output: retrieved
        - kind: set
          values: { phase: answer }
```

Important fields:

- `inputs` and `outputs` are the typed module boundary.
- `state` is private workflow state.
- `requires.capabilities` declares the permissions the module needs.
- `tools` declares exact tool names and optional provider capabilities.
- `workflow.max_steps` bounds execution.
- `policy` can add budgets such as `max_tool_calls`, `max_model_calls`, `timeout_seconds`, and `require_approval`.

Object schemas allow undeclared fields by default for compatibility with provider metadata and evolving module contracts. Add `additional_properties: false` to a detailed object schema when the AIR VM and generated strict backends should reject undeclared fields.

Conditions use AIR's small equality-only condition DSL. `&&` binds tighter than `||`; parentheses and numeric comparisons are not supported. See [condition_dsl.md](condition_dsl.md) for the formal grammar and limits.

## 3. RunPlan Anatomy

A RunPlan connects modules into an app:

```yaml
plan:
  name: simple-helpdesk
  version: 0.1.0

requires:
  capabilities: [retrieval.local]

nodes:
  - id: helpdesk
    module: helpdesk.rag@0.1.0

entry: helpdesk

connect:
  - from: $input.question
    to: helpdesk.question

outputs:
  answer: helpdesk.answer
```

Key rules:

- `nodes[].module` must exist in the module store.
- `connect` maps `$input` or upstream module outputs into module inputs.
- `outputs` maps final plan outputs to module outputs.
- `requires.capabilities` must cover every static and dynamic module.
- `halts` can stop a plan for clarification or approval-style flows.
- `schedule.groups` can declare bounded parallel waves.

For deterministic shape changes, `connect.value` supports typed expression transforms such as
object construction, arrays, counts, and coalescing. For semantic or lossy interface conversion,
make the conversion an explicit adapter module with a `model_call`, then connect source output to
the adapter input and adapter output to the target module. This keeps model cost, timeout, retry,
schema validation, and trace events visible instead of hiding model execution inside wiring.

## 4. Module Store And Recipes

A module store publishes modules and optional recipes:

```yaml
store:
  name: simple-helpdesk-store
  version: 0.1.0

modules:
  helpdesk.rag@0.1.0:
    path: examples/simple-helpdesk/helpdesk-rag.air.yaml
    kind: composite
    visibility: public
    description: "Retrieve local helpdesk documents, then answer with citations."
    covers: [retrieve_docs, answer_question]
    priority: 100
```

Planner behavior:

- public composites are shown before internal primitives;
- `priority` and `covers` help the planner choose large verified components;
- recipes let the planner select a pre-validated topology by `recipe_id`;
- selected recipes are materialized locally and validated before execution.

## 5. Dynamic Fan-Out

Use `dynamic.fanouts` when a module emits a typed array at runtime:

```yaml
dynamic:
  fanouts:
    - id: topic_wave
      after: plan
      source: plan.plan.topics
      module: deep_research.research_topic@0.1.0
      id_prefix: topic
      min_items: 1
      max_items: 4
      max_parallel: 4
      input:
        - from: plan.plan.research_brief
          to: $each.research_brief
        - from: $item
          to: $each.topic
      fan_in:
        - from: $each.note
          to: final.notes[]
```

This is AIR's replacement for arbitrary runtime graph mutation. The array source, module, bounds, inputs, and fan-in are declared and validated.

## 6. Model Config

AIR uses OpenAI-compatible model config for real model calls:

```json
{
  "models": {
    "rag_answerer": {
      "base_url": "https://example.com/v1",
      "base_url_env": "OPENAI_BASE_URL",
      "api_key_env": "BIGMODEL_API_KEY",
      "model": "example-model",
      "model_env": "OPENAI_MODEL",
      "temperature": 0,
      "request_timeout_seconds": 120,
      "system_prompt": "Return only JSON."
    }
  }
}
```

`request_timeout_seconds` is optional and defaults to 120 seconds. AIR action `timeout_seconds` is forwarded to timeout-aware providers, and the OpenAI-compatible provider uses the smaller of the provider request timeout and the AIR action timeout for each HTTP request. AIR also records elapsed-time violations in the runtime trace.

Run with:

```bash
export BIGMODEL_API_KEY=...
export OPENAI_BASE_URL=https://example.com/v1
export OPENAI_MODEL=example-model
cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
```

Generated OpenAI JS strict runtimes read the same model config.

## 7. Tool Config And Capabilities

Tools are declared in the module and configured at runtime.

Module declaration:

```yaml
tools:
  - name: docs.search
    capability: retrieval.local

requires:
  capabilities: [retrieval.local]
```

Tool config:

```json
{
  "tools": {
    "docs.search": {
      "kind": "local_docs_search",
      "capability": "retrieval.local",
      "max_results": 3,
      "documents": [
        {
          "id": "refund",
          "title": "Refund policy",
          "content": "Refund requests are routed to billing support."
        }
      ]
    }
  }
}
```

AIR checks:

- the module declares the tool;
- the tool capability is in `requires.capabilities`;
- the provider-configured capability matches the module-declared capability;
- `policy.max_tool_calls` is not exceeded.

Common native tools live in the `air-tools` crate and are configured through `--tool-config`.

| Tool kind | Typical AIR tool name | Input | Output | Notes |
| --- | --- | --- | --- | --- |
| `local_docs_search` | `docs.search` | `{ "query": "..." }` | `{ query, documents[] }` | In-memory docs for examples, tests, and local RAG. |
| `local_reflection` | `research.think` | `{ "reflection": "..." }` | `{ reflection }` | Deterministic reflection placeholder for bounded research loops. |
| `http_json` | `web.search` / custom API | tool input object | JSON response body | Calls REST endpoints; response must be JSON. |
| `web_fetch` | `web.fetch` | `{ "url": "https://..." }` | `{ url, status, content_type, text, bytes, truncated }` | GET only; supports headers, bearer token env, timeout, max bytes. |
| `file_read` | `file.read` | `{ "path": "relative/file.txt" }` | `{ path, content, bytes, truncated }` | Read-only and constrained to configured `base_dir`. |
| `git_diff` | `git.diff` | `{ "path": "...", "staged": false }` | `{ repo, diff, bytes, truncated }` | Read-only diff; path filters must stay inside `repo_dir`. |

For production-shaped adapters, AIR also supports an HTTP JSON tool provider in the native VM:

```json
{
  "tools": {
    "web.search": {
      "kind": "http_json",
      "capability": "network.search",
      "url": "https://search.example.com/query",
      "method": "POST",
      "bearer_token_env": "SEARCH_API_KEY",
      "timeout_seconds": 10,
      "body": {
        "query": "{{query}}"
      }
    }
  }
}
```

`body`, `url`, and header values can use `{{field}}` templates from the tool input. The response must be JSON and must match the module state/output schema for the action target.

Example read-only file and git tools:

```json
{
  "tools": {
    "file.read": {
      "kind": "file_read",
      "capability": "file.read",
      "base_dir": ".",
      "max_bytes": 262144
    },
    "git.diff": {
      "kind": "git_diff",
      "capability": "code.read",
      "repo_dir": ".",
      "max_bytes": 262144
    }
  }
}
```

## 8. Approvals

Dangerous capabilities can require explicit approval:

```yaml
policy:
  require_approval: [production.deploy]
```

The workflow must contain an approval action before using that capability:

```yaml
- kind: approval
  approval_for: [production.deploy]
```

Runtime approval config:

```json
{
  "tools": {},
  "approvals": {
    "production.deploy": {
      "approved": true,
      "approver": "release-manager",
      "reason": "approved deployment window"
    }
  }
}
```

Default behavior is fail-closed. If approval is missing or denied, execution stops and the trace records the failure.

## 9. Traces, Checkpoints, Resume

Use logs for humans:

```bash
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml --log
```

Use JSONL traces for replay/audit. Trace files are redacted by default: common sensitive keys such as API keys, authorization headers, passwords, secrets, and tokens are masked, and large string/event payloads are capped.

```bash
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml \
  --trace-out target/generated/run.trace.jsonl
```

Use raw traces only for trusted local debugging. Raw traces preserve complete model/tool inputs and outputs, which can include prompts, documents, credentials, or user data.

```bash
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml \
  --trace-out target/generated/run.raw.trace.jsonl \
  --trace-raw
```

Use checkpoint/resume for long plans:

```bash
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml \
  --checkpoint-out target/generated/run.state.json

cargo run -p air-cli -- resume-plan --profile examples/deep-research/profile.air-profile.yaml \
  --state target/generated/run.state.json \
  --override clarify.clarification.normalized_question='"Updated question"'
```

Use JIT hot-path cache for stable dynamic topologies:

```bash
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml \
  --jit-cache target/generated/jit_cache
```

AIR specializes the resolved topology, validates it, and reuses the static hot path for matching inputs. It does not cache model outputs.

## 10. Lowering To Backends

Lower a validated RunPlan to LangGraph:

```bash
cargo run -p air-cli -- lower-plan examples/deep-research/deep-research-dynamic.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --backend langgraph \
  --output target/generated/deep_research.langgraph.py
```

Lower to OpenAI JS strict:

```bash
cargo run -p air-cli -- lower-plan examples/deep-research/deep-research-dynamic.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/deep_research.openai.mjs
```

Generated host contracts:

- LangGraph exposes `AIR_TOOL_PROVIDER`, `AIR_TOOL_CAPABILITIES`, and `AIR_APPROVAL_PROVIDER`.
- OpenAI JS strict reads model config and tool config from CLI flags.
- `AIR_TRACE=1` emits AIR JSONL trace events on stderr.
- provider logs go to stderr so stdout stays machine-readable JSON.

## 11. Verification

The current 1.0 gate is:

```bash
scripts/verify_1_0.sh
```

For the real-model extension:

```bash
AIR_1_0_REAL=1 scripts/verify_1_0.sh
```
