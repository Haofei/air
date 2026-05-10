# AIR

**Composable Agent IR: compile, link, run, and observe bounded agents across runtimes.**

AIR is a compiler-first module system for production agent workflows. Agents are written as typed, bounded state machines in `.air.yaml`; applications compose them with verified RunPlans; runtimes execute the checked graph on the native Rust VM or generated backend code.

## Why AIR

AIR's core value is not merely "agents can run". It is "after an AI acted, you can inspect what
actually happened". The native VM, traces, typed tool contracts, budgets, and RunPlan validation are
designed around that audit surface. opencode optimizes for developer freedom; AIR optimizes for
bounded freedom with guarantees.

The end state is not an AI that rewrites its own constitution at runtime. It is a stable AIR
constitution that lets coding agents continuously improve modules, tools, prompts, and backends
through auditable traces, tests, benchmarks, and reviewed changes.

| Problem | AIR |
| --- | --- |
| Agent code is locked to one SDK | One checked IR can run on the AIR VM or lower to LangGraph / OpenAI JS strict runtimes |
| Multi-agent flows are prompt-shaped | RunPlans connect typed module inputs and outputs with static validation |
| Dynamic agent graphs are hard to audit | Dynamic fan-out/fan-in is bounded, typed, traced, and can be specialized |
| Tool permissions are implicit | Capabilities, provider tool contracts, budgets, and approval gates are verified |
| Long runs are opaque | Human logs, JSONL traces, checkpoints, resume, and replay are built in |

For coding agents, that means the important questions have first-class places to live:

| Question | AIR audit surface |
| --- | --- |
| Which files changed? | `file.edit`, `file.patch`, `file.write`, repair `workspace_diff`, and repair baseline fields distinguish agent changes from pre-existing dirty files |
| Why did the model make the change? | `model_call` inputs and outputs are traced, redacted by default |
| What evidence was cited? | Search/context tools return source ids, and compaction/report modules carry those ids forward |
| Did it run a risky command? | `command_run` is exposed through allowlisted command templates, not raw shell access |
| Did it exceed the budget or repeat itself? | `max_tool_calls`, `max_model_calls`, `max_repeated_tool_calls`, timeouts, and capability gates are checked by the runtime |
| What will this recipe be allowed to do? | `air code --explain` shows the resolved profile, RunPlan, capabilities, and write permission before execution |

## Status

AIR is usable as a local compiler/runtime prototype with bounded production-oriented semantics. Backend lowering is available but still experimental; the native Rust VM is the reference runtime.

| Feature | Status |
| --- | --- |
| Typed state-machine AIR modules | Supported |
| Module schema validation | Supported, including opt-in strict object fields with `additional_properties: false` |
| Model/tool providers and capability checks | Supported; common native tools live in `air-tools`; `tool_dispatch` lets a model select among declared tools without bypassing AIR capability/budget checks |
| Reusable standard modules | Supported; `modules/std` includes generic context compaction |
| Approval gates, retry, budgets, repeated-tool guards, and action timeouts | Supported; runtime forwards action deadlines to timeout-aware providers |
| RunPlan module composition | Supported |
| Dynamic bounded fan-out/fan-in | Supported |
| Checkpoint, halt/resume, replay, and JIT hot-path specialization | Supported |
| LangGraph strict lowering | Experimental |
| OpenAI Agents JS strict lowering | Experimental |
| OpenAI Agents JS semantic lowering | Limited: one module shape, local-docs RAG oriented |
| Hard cancellation of arbitrary synchronous providers | Not yet; OpenAI/http-json providers enforce request deadlines |
| Trace redaction and sensitive-field policy | Supported: trace files are redacted by default; use `--trace-raw` only in trusted debug runs |
| Backend conformance suite | Supported for core strict-runtime cases via `scripts/verify_backend_conformance.sh` |

## Quick Start

Run the small packaged example first:

```bash
cargo build
cargo run -p air-cli -- validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml

export BIGMODEL_API_KEY=...
export BIGMODEL_BASE_URL=https://open.bigmodel.cn/api/coding/paas/v4
export BIGMODEL_MODEL=GLM-5.1
cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

The profile packages a RunPlan, module store, input, model config, and tool config. It is the recommended shape for user-facing AIR apps.

## Examples

The repository intentionally keeps examples focused:

| Example | Purpose |
| --- | --- |
| `examples/simple-helpdesk/` | One-agent RAG workflow with local document search, model call, typed output, and provider capability check |
| `examples/deep-research/` | Multi-agent research workflow with clarification, planning, bounded fan-out, fan-in, resume, parallel execution, and backend lowering |
| `examples/code-agent/` | Bounded coding workflow with exploration, review, repair, page build, context compaction, constrained tools, and patch audit traces |

The shared OpenAI-compatible model config lives at `examples/bigmodel-openai-compatible.json`.

## What An Agent Looks Like

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

    - id: answer
      when: phase == "answer"
      actions:
        - kind: model_call
          model: rag_answerer
          input: { object: { question: { ref: question }, retrieved: { ref: retrieved } } }
          output: answer
        - kind: set
          values: { phase: done }
```

## RunPlan Composition

A RunPlan links modules into an application:

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

Validate and run it:

```bash
cargo run -p air-cli -- validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --trace-out target/generated/simple.trace.jsonl
cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --trace-out target/generated/simple.raw.trace.jsonl --trace-raw
```

## Deep Research

Deep research is the main 1.0 proof case. It maps a LangGraph-style research app into AIR without arbitrary runtime `goto`.

```bash
cargo run -p air-cli -- validate-plan --profile examples/deep-research/profile.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml --log
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml --parallel
```

It demonstrates:

- clarification and halt/resume;
- static DAG composition;
- dynamic bounded fan-out and array fan-in;
- nested dynamic fan-out;
- schedule groups and opt-in parallel execution;
- model/tool call budgets and action timeouts;
- schema retry and token-limit compaction retry;
- reusable context-budget measurement and semantic compaction module;
- local tool capability handshakes;
- checkpoint/resume;
- JIT hot-path specialization;
- lowering to LangGraph and OpenAI JS strict runtimes.

## Backend Portability

Lower a checked RunPlan to another backend:

```bash
cargo run -p air-cli -- lower-plan examples/deep-research/deep-research-dynamic.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --backend langgraph \
  --output target/generated/deep_research.langgraph.py

cargo run -p air-cli -- lower-plan examples/deep-research/deep-research-dynamic.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/deep_research.openai.mjs
```

The native VM is the conformance runtime. Generated backends preserve AIR schema validation, retry inputs, dynamic fan-out runtime, trace events, tool contracts, and approval host hooks.

Because tools are resolved at the profile/runtime level rather than in the IR, tool implementations can be compiled to WASM and shared across backends. A tool compiled to a `.wasm` module needs no per-platform rewrite: the AIR runtime loads it through the same `ToolProvider` interface used by native Rust tools today.

## CLI Surface

Primary user-facing commands:

| Command | Description |
| --- | --- |
| `plan` | Ask a planner model to select modules and generate a bounded RunPlan |
| `validate-plan` | Verify a `.air-plan.yaml` against a module store or profile |
| `run-plan` | Execute a RunPlan or packaged profile on the native VM |
| `resume-plan` | Resume a halted/checkpointed plan with typed overrides |
| `lower-plan` | Compile a checked RunPlan to another backend |
| `code` | User-facing coding-agent wrapper with deterministic recipe routing, `--explain` permission preflight, and bounded `--loop` iteration |

Lower-level module, system, and trace commands exist for development and tests, but are hidden from default help output.

## Verification

Run the 1.0 gate:

```bash
scripts/verify_1_0.sh
```

Add real model calls through the OpenAI-compatible config:

```bash
AIR_1_0_REAL=1 scripts/verify_1_0.sh
```

## Documentation

- [AIR User Guide](docs/user_guide.md)
- [Open Deep Research Migration Notes](docs/open_deep_research_migration.md)
- [AIR 1.0 Audit](docs/air_1_0_audit.md)
