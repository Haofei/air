# AIR 1.0 Audit

This document defines the current AIR 1.0 bar and tracks evidence against it.

AIR 1.0 is not "all possible agent features." The 1.0 target is:

- a typed, bounded agent IR;
- a verified RunPlan composition layer;
- a native VM that is the conformance runtime;
- dynamic behavior through explicit bounded primitives rather than arbitrary `goto`;
- traceable execution with replay/specialization;
- enough backend portability to prove the IR is not locked to one SDK;
- one realistic end-to-end case, deep research, that runs through real model/tool paths.

## Current Estimate

Current completion: **1.0 scope implemented for the agreed bounded-agent target**.

The technical core, user documentation, external-provider smoke path, diagnostics for common config failures, and final release audit gate are now in place. The remaining items below are post-1.0 hardening unless the release bar changes.

## Success Criteria

| Area | 1.0 bar | Current evidence | Status |
| --- | --- | --- | --- |
| Agent IR | Modules are typed bounded state machines with explicit inputs, outputs, state, tools, requirements, policy, and observability. | `crates/air-core`, `crates/air-verify`; examples under `examples/deep-research/*.air.yaml`; tests in `crates/air-verify/tests/verify.rs`. | Done |
| Composition IR | RunPlan supports DAG composition, typed connections, outputs, halts, schedules, capabilities, dynamic fan-out, and nested bounded fan-out. | `crates/air-linker`; `examples/deep-research/*.air-plan.yaml`; `tests/plans/dynamic-*.air-plan.yaml`; linker tests. | Done |
| Native VM | Runs checked state-machine modules and RunPlans with model/tool calls, schema validation, retry, token-limit retry, timeout, budgets, approval, trace, checkpoint, resume. | `crates/air-runtime`; `crates/air-linker`; `cargo test`; `scripts/verify_deep_research.sh`. | Done |
| Dynamic behavior | Supports bounded fan-out/fan-in, conditional routes, bounded loops, halt/resume, checkpoint/resume, and JIT hot-path specialization without bypassing validation. | `dynamic.fanouts`, `when`, state-machine `max_steps`, `halts`, `resume-plan`, `run-plan --jit-cache`; tests and verifier script. | Done |
| Planner/module store | Planner sees ranked recipes/composites before primitives; recipe selection materializes locally and validates before execution. | `module-store.air-store.yaml`, planner request tests in `crates/air-cli/src/main.rs`, recipe examples. | Done |
| Tools/capabilities | Native and generated runtimes reject undeclared tools and provider capability mismatches; tool config supports local docs search, reflection, and HTTP JSON tools. | `ToolProvider::tool_capability`; `tests/plans/tool-capability-smoke.*`; `tests/plans/http-json-tool-smoke.*`; generated backend smoke in `scripts/verify_deep_research.sh`; HTTP provider smoke in `scripts/verify_1_0.sh`. | Done |
| Approval | Default fail-closed approval, explicit approval decisions in native CLI config, generated LangGraph/OpenAI JS approval host contracts, trace events. | `ApprovalDecision`; `tests/plans/approval-smoke.*`; generated approval smoke in verifier. | Done |
| Backend portability | Lower checked RunPlans to LangGraph and OpenAI JS strict runtimes with AIR schema checks, dynamic fan-out runtime, retry compaction, trace, tools, approvals. | `crates/air-backend-langgraph`, `crates/air-backend-openai-agents-js`, `scripts/verify_backend_conformance.sh`, verifier generated backend smoke. | Good enough for 1.0 |
| Realistic app | Deep research maps the LangGraph app pattern into AIR and runs with real model/tool paths. | `docs/open_deep_research_migration.md`; `examples/deep-research`; optional `AIR_DEEP_RESEARCH_REAL=1`. | Good enough for 1.0 |
| Public CLI | Default help exposes focused commands: `plan`, `validate-plan`, `run-plan`, `resume-plan`, `lower-plan`. | `cargo run -p air-cli -- --help`; CLI tests. | Done |
| Observability | Human logs and JSONL traces include action status, input/output, retry metadata, dynamic materialization, approvals. | `--log`, `--trace-out`, generated `AIR_TRACE=1`; verifier trace assertions. | Done |
| Release docs | New user can understand module/store/profile/tool-config/backends/traces without reading source. | README, `docs/user_guide.md`, deep research migration audit. | Done |
| Provider ecosystem | At least one realistic external provider beyond local docs/reflection is documented or implemented. | OpenAI-compatible model provider; local docs/reflection tools; native HTTP JSON tool provider with local server smoke. | Done |
| Diagnostics | Common failures produce concise actionable messages. | Input/profile parsing context; model config field validation; tool config field validation; verifier diagnostics; runtime/provider messages. | Done |
| Packaged examples | Public examples are focused and runnable: one simple app plus deep research. | `examples/simple-helpdesk`, `examples/deep-research`, `examples/README.md`; both profiles are validated by `scripts/verify_1_0.sh`. | Done |
| Release gate | One documented command proves the 1.0 scope, with optional real-model extension. | `scripts/verify_1_0.sh` wraps public CLI/docs checks, packaged example validation, HTTP JSON provider smoke, and `scripts/verify_deep_research.sh`. | Done |

## Post-1.0 Hardening

These are useful next steps, but they are not required for the scoped 1.0 target.

### 1. More Runtime Diagnostics

The 1.0 CLI now validates common config failures before execution:

- invalid model config path/JSON/fields;
- empty model alias set;
- invalid tool config path/JSON/fields;
- non-object `--input`;
- invalid run profile path/YAML.

Further polish can make runtime provider failures more structured:

- missing API key;
- schema mismatch;
- capability mismatch;
- invalid RunPlan connection;
- generated backend host hook missing.

### 2. Gate Expansion

`scripts/verify_1_0.sh` is the current release gate. Future releases can expand it with:

- any additional public docs checks;
- optional `cargo clippy` if warnings are kept under control;
- any new provider example smoke.

## Not Required For 1.0

These are explicitly post-1.0 unless the scope changes:

- arbitrary LangGraph-style `Command(goto=...)`;
- unbounded autonomous loops;
- full MCP auth/discovery;
- provider-native search parity across OpenAI/Anthropic/Tavily;
- interactive human approval queues;
- preemptive cancellation of in-flight provider calls;
- evaluator parity with `open_deep_research`;
- making parallel execution default.

## Current Gate

The current gate is:

```bash
scripts/verify_1_0.sh
```

For the optional real-model extension:

```bash
AIR_1_0_REAL=1 scripts/verify_1_0.sh
```

The last local deep verifier run passed after adding:

- native and generated approval smokes;
- native and generated tool capability smokes;
- LangGraph host tool capability checks;
- OpenAI JS strict stdout/stderr separation for machine-readable output;
- native HTTP JSON tool provider smoke;
- model/tool/input/profile diagnostic tests;
- simple-helpdesk and deep-research packaged profile validation.
