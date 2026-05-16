# AIR

**Composable Agent IR: compile, link, run, and observe bounded agents on a native VM.**

AIR is a compiler-first module system for production agent workflows. Agents are written as typed, bounded state machines in `.air.yaml`; applications compose them with verified RunPlans; the native Rust VM executes the checked graph and records auditable traces.

## Why AIR

AIR's core value is not merely "agents can run". It is "after an AI acted, you can inspect what
actually happened". The native VM, traces, typed tool contracts, budgets, and RunPlan validation are
designed around that audit surface. opencode optimizes for developer freedom; AIR optimizes for
bounded freedom with guarantees.

The end state is not an AI that rewrites its own constitution at runtime. It is a stable AIR
constitution that lets coding agents continuously improve modules, tools, prompts, and runtime adapters
through auditable traces, tests, benchmarks, and reviewed changes.

| Problem | AIR |
| --- | --- |
| Agent code is locked to one SDK | One checked IR runs on the native AIR VM behind a stable module and tool contract |
| Multi-agent flows are prompt-shaped | RunPlans connect typed module inputs and outputs with static validation |
| Dynamic agent graphs are hard to audit | Dynamic fan-out/fan-in is bounded, typed, traced, and can be specialized |
| Tool permissions are implicit | Capabilities, provider tool contracts, budgets, and approval gates are verified |
| Long runs are opaque | Human logs, JSONL traces, checkpoints, resume, and replay are built in |

For coding agents, that means the important questions have first-class places to live:

| Question | AIR audit surface |
| --- | --- |
| Which files changed? | `edit`, bash-run `git diff`, and edit-loop `workspace_diff` record the audited workspace change |
| Why did the model make the change? | `model_call` inputs and outputs are traced, redacted by default |
| What evidence was cited? | Search/context tools return source ids, and compaction/report modules carry those ids forward |
| Did it run a risky command? | `command_run` is exposed through allowlisted command templates, not raw shell access |
| Did it exceed the budget or repeat itself? | `max_tool_calls`, `max_model_calls`, `max_repeated_tool_calls`, timeouts, and capability gates are checked by the runtime |
| What will this agent be allowed to do? | `air code --explain` shows the resolved profile, RunPlan, capabilities, and write permission before execution |

## Status

AIR is usable as a local compiler/runtime prototype with bounded production-oriented semantics. The native Rust VM is the reference runtime. Generated backend lowering has been removed and can return later as a separate compatibility layer.

| Feature | Status |
| --- | --- |
| Typed state-machine AIR modules | Supported |
| Module schema validation | Supported, including opt-in strict object fields with `additional_properties: false` |
| Model/tool providers and capability checks | Supported; common native tools live in `air-tools`; bounded `tool_batch_dispatch` lets a model select declared tools without bypassing AIR capability/budget checks |
| Reusable standard modules | Supported; `modules/std` includes generic context compaction |
| Approval gates, retry, budgets, repeated-tool guards, and action timeouts | Supported; runtime forwards action deadlines to timeout-aware providers |
| RunPlan module composition | Supported |
| Dynamic bounded fan-out/fan-in | Supported |
| Checkpoint, halt/resume, replay, and JIT hot-path specialization | Supported |
| Hard cancellation of arbitrary synchronous providers | Not yet; OpenAI/http-json providers enforce request deadlines |
| Trace redaction and sensitive-field policy | Supported: trace files are redacted by default; use `--trace-raw` only in trusted debug runs |

## Quick Start

Run the small packaged example first:

```bash
cargo build
cargo run -p air-cli -- validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml

cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

`air` now auto-loads a repository-root `.env`. Put provider settings there:

```dotenv
OPENAI_API_KEY=...
OPENAI_BASE_URL=https://open.bigmodel.cn/api/coding/paas/v4
OPENAI_MODEL=GLM-5.1
```

The profile packages a RunPlan, module store, input, model config, and tool config. It is the recommended shape for user-facing AIR apps.

## Examples

The repository intentionally keeps examples focused:

| Example | Purpose |
| --- | --- |
| `examples/simple-helpdesk/` | One-agent RAG workflow with local document search, model call, typed output, and provider capability check |
| `examples/deep-research/` | Multi-agent research workflow with clarification, planning, bounded fan-out, fan-in, resume, and parallel execution |
| `examples/code-agent/` | Bounded coding workflow with exploration, review, one unified edit loop, context compaction, constrained tools, and patch audit traces |

The shared OpenAI-compatible model config lives at `examples/bigmodel-openai-compatible.json` and defaults to `OPENAI_API_KEY`, `OPENAI_BASE_URL`, and `OPENAI_MODEL`.

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

Deep research is the main research proof case. It maps a LangGraph-style research app into AIR without arbitrary runtime `goto`.

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
- native trace/replay and JIT specialization.

## CLI Surface

Primary user-facing commands:

| Command | Description |
| --- | --- |
| `plan` | Ask a planner model to select modules and generate a bounded RunPlan |
| `validate-plan` | Verify a `.air-plan.yaml` against a module store or profile |
| `run-plan` | Execute a RunPlan or packaged profile on the native VM |
| `resume-plan` | Resume a halted/checkpointed plan with typed overrides |
| `code` | Minimal coding-agent wrapper around the default read/search/edit/verify AIR loop, with `--explain` permission preflight |

Lower-level module, system, and trace commands exist for development and tests, but are hidden from default help output.

## Verification

Run the workspace checks:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Targeted example smoke checks live with their examples when you need them:

```bash
bash examples/deep-research/dev/verify.sh
```

## Documentation

- [AIR User Guide](docs/user_guide.md)
- [Condition DSL](docs/condition_dsl.md)
- [Open Deep Research Migration Notes](docs/open_deep_research_migration.md)

## Roadmap

- [ ] **Whole-program compilation (merge + flatten).** The linker currently composes modules into a plan but preserves module boundaries at runtime. A merge compiler would flatten a multi-module plan into a single state machine with a unified state schema, resolved field names, and merged policies. This simplifies analysis, replay, and any future execution targets by removing module dispatch from the hot path. Analogous to LLVM LTO or TensorFlow XLA: separate compilation for development, whole-program compilation for output.
- [ ] **Module registry.** A publish/install system for sharing agent modules across projects. Module stores are currently local files; a registry would let teams publish versioned modules (`air publish context.compact@0.2.0`) and consume them via dependency declarations, enabling a shared standard library of reusable agent components.
- [ ] **Schema conformance testing.** A lightweight test harness that calls real LLMs but only validates output structure against the declared AIR schema—no assertion on specific content. This catches the most common production failure mode (LLM returning wrong shapes) without brittle mock providers. The runtime already has `validate_output`; the test layer just needs a harness that runs a module's model calls against a real provider and reports schema violations per phase.
