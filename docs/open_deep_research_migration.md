# Open Deep Research Migration Audit

This document tracks how AIR maps the LangGraph `open_deep_research` app without copying LangGraph's runtime model directly.

## Objective

The migration should prove that AIR can express the product pattern behind deep research:

- turn a user research request into a structured brief;
- split the brief into bounded research topics;
- run multiple focused researchers over tools;
- compress each researcher's findings;
- synthesize a final cited report;
- validate module boundaries and generated topology before execution;
- replay and specialize dynamic traces back into checked AIR RunPlans.

The goal is not a line-for-line port of LangGraph internals. AIR should keep modules static, typed, bounded, and auditable. Dynamic selection belongs in the planner/linker layer that emits a verified RunPlan.

## Source App Shape

The cloned source lives at `target/research/open_deep_research`.

Relevant source files:

- `src/open_deep_research/deep_researcher.py`
- `src/open_deep_research/state.py`
- `src/open_deep_research/configuration.py`
- `src/open_deep_research/utils.py`

LangGraph workflow:

| Source behavior | Source node or type | AIR mapping |
| --- | --- | --- |
| Optional clarification | `clarify_with_user` and `ClarifyWithUser` | `deep_research.clarify_scope@0.1.0` normalizes the question, records assumptions, and reports whether clarification is needed before bounded planning. RunPlan `halts` can stop after this module and return the clarification request. AIR linker resume can continue the same RunPlan from saved module outputs after the caller applies a typed endpoint override with the user's answer. |
| Research brief generation | `write_research_brief` and `ResearchQuestion` | `deep_research.plan_array@0.1.0` emits `research_brief`. |
| Supervisor planning | `supervisor` using `ConductResearch`, `ResearchComplete`, `think_tool` | AIR planner generates a bounded RunPlan; AIR modules do not create arbitrary graph nodes at runtime. The supervised AIR module emits a typed `action` enum (`conduct_research` or `research_complete`), bounded `follow_up_topics`, and records its rationale through the explicit `research.think` reflection tool. |
| Parallel delegated researchers | `supervisor_tools` invokes `researcher_subgraph` up to `max_concurrent_research_units` | RunPlan instantiates `deep_research.research_topic@0.1.0` once per selected topic. `schedule.groups` records bounded research waves and their `max_parallel` budget; the AIR VM currently emits schedule trace events while executing modules sequentially through the synchronous provider traits. The supervised examples keep optional waves statically present and use node `when` conditions to skip or run them; when a wave runs, its topic inputs come from the previous supervisor's `decision.follow_up_topics[]`. |
| Researcher ReAct loop | `researcher` and `researcher_tools` loop until complete or `max_react_tool_calls` | Current AIR module uses a bounded state-machine loop: search, model-selected follow-up decision, explicit `research.think` reflection tool call, conditional route, up to two follow-up searches, then compression. Each search result is accumulated with the AIR `append` action into typed state before compression. If the model still says more research is needed at the bound, the module compresses the bounded evidence collected so far instead of mutating the graph or failing open. The loop is capped by `max_steps`, `policy.max_tool_calls`, and `policy.max_model_calls`. |
| Search and MCP tools | `get_all_tools`, Tavily/OpenAI/Anthropic/MCP | Current example uses `web.search` capability with tool config. Provider-specific search and MCP auth are future tool bindings. |
| Per-topic compression | `compress_research` | `deep_research.research_topic@0.1.0` compresses search results into a typed `note`. |
| Final report | `final_report_generation` | `deep_research.final_report@0.1.0` produces typed `final_report`. |
| Reducers | `override_reducer`, `operator.add` | AIR supports array append fan-in via `to: final.notes[]`. |
| Runtime routing | LangGraph `Command(goto=...)` | AIR keeps module-internal state machines static and uses RunPlan DAG edges externally. |

## AIR Artifacts

Core files:

- `examples/deep-research/module-store.air-store.yaml`
- `examples/deep-research/clarify-scope.air.yaml`
- `examples/deep-research/research-plan-array.air.yaml`
- `examples/deep-research/research-topic.air.yaml`
- `examples/deep-research/final-report.air.yaml`
- `examples/deep-research/deep-research-clarified.air-plan.yaml`
- `examples/deep-research/deep-research-array.air-plan.yaml`
- `examples/deep-research/deep-research-supervised.air-plan.yaml`
- `examples/deep-research/deep-research-supervised-two-step.air-plan.yaml`
- `examples/deep-research/profile.air-profile.yaml`

Generated evidence:

- `target/generated/deep_research_planned_array.air-plan.yaml`
- `target/generated/deep_research_planned_current.air-plan.yaml`
- `target/generated/deep_research_planned_current.output.json`
- `target/generated/deep_research_planned_current.trace.jsonl`
- `target/generated/deep_research_supervised_real.output.json`
- `target/generated/deep_research_supervised_real.trace.jsonl`

The important design point is that users do not author `topic_1`, `topic_2`, or `topic_4` by hand. Users provide a task and a module store. `air plan` selects modules and emits the bounded RunPlan. The verifier checks the generated graph before execution.

The module store also exposes a planning recipe, `deep_research.four_topic_report@0.1.0`, for the common four-topic workflow. Recipes are validated RunPlan templates in the store catalog. They let the planner choose a larger component/topology when it matches the task, instead of assembling every small module from scratch. `air plan` accepts short recipe selections and materializes the full RunPlan locally from the store, so the model does not have to copy or invent the topology.

## Dynamic Boundary

AIR should not copy LangGraph's fully open `Command(goto=...)` model. The production dynamic needs for agent systems are narrower and easier to verify:

| Dynamic need | AIR shape |
| --- | --- |
| Unknown number of subtasks | Bounded fan-out over typed items, then typed array fan-in |
| Unknown route | Declared `when` conditions over typed state |
| Unknown need for another round | Bounded state-machine or RunPlan loop with policy limits and conditional exit |
| Unknown need for user input | Typed `halt` / `resume` with endpoint overrides |

The remaining dynamic layer should be an admission-controlled interpreter, not arbitrary runtime graph mutation. Each dynamic decision must stay inside declared capabilities, allowed modules, schema-compatible inputs/outputs, and policy budgets.

JIT should specialize traces, not bypass AIR validation:

1. first runs execute through the dynamic interpreter and record chosen modules, fan-out cardinality, routes, schemas, policies, and module versions;
2. stable hot paths are materialized into ordinary `.air-plan.yaml` DAGs;
3. the generated RunPlan is accepted only after `validate-plan` succeeds against the current module store;
4. cache keys include module version/schema/policy identity, so stale JIT plans are invalidated instead of silently reused.

## Current AIR Coverage

Implemented:

- module store with public planning/reporting modules and an internal reusable researcher module;
- explicit scope clarification/normalization module before planning;
- AIR RunPlan halt semantics for clarification gates, so a plan can return `clarification` instead of continuing into research when `needs_clarification` is true;
- `RunStatus::Halted` / `RunStatus::Completed` in AIR linker results, so callers can distinguish a clarification stop from a completed report without inspecting output keys;
- AIR linker resume from halted module outputs with endpoint overrides, e.g. replacing `clarify.clarification.normalized_question` with the user's clarified scope before continuing downstream;
- AIR linker checkpoints after every completed RunPlan module, exposed in the CLI as `run-plan --checkpoint-out` / `resume-plan --checkpoint-out` using the same JSON state format as `resume-plan --state`; dynamic fan-out checkpoints can resume after the planner or after partially completed materialized researcher nodes without rerunning completed nodes;
- planner-generated RunPlan with multiple instantiations of the same researcher module;
- `dynamic.fanouts` on RunPlan, so planner or supervisor modules can emit typed arrays at runtime and AIR can materialize bounded researcher waves from `source`, `$item`, `$each`, `max_items`, and typed `fan_in` mappings;
- multiple independent dynamic fan-out boundaries in one RunPlan, so later static modules can emit additional typed arrays and AIR can materialize those waves before final fan-in;
- nested dynamic fan-out in the constrained form `after: <parent_fanout_id>` plus `source: $parent.<array_output>`, so each materialized parent node can emit a bounded child array without opening arbitrary runtime `goto`;
- module-store planning recipes for larger reusable topologies, including a four-topic deep-research report recipe;
- planner recipe-selection protocol, where the model may return `recipe_id` and AIR materializes the verified RunPlan locally;
- planner component-selection ranking, where AIR scores recipes, public composites, and lower-level modules before the model sees the catalog so the planner receives a first-choice large component instead of a flat bag of primitives;
- RunPlan schedule groups with `max_parallel`, so bounded fan-out waves such as four topic researchers or supervised follow-up researchers are represented explicitly and validated instead of being implicit in topological order;
- explicit AIR linker parallel runner for schedule groups and dynamic fan-out waves, using per-worker provider factories so independent nodes can execute concurrently without sharing mutable provider state; `run-plan --parallel` exposes this as an opt-in CLI path while default execution remains sequential;
- trace specialization API that extracts the resolved RunPlan from execution trace, preferring a materialized dynamic hot path when present, revalidates it against the current module store, and emits a cache identity containing module schemas, requirements, and policies;
- `run-plan --jit-cache <dir>` hot-path cache, where AIR keys a specialized RunPlan by the original plan, module store, module contents, and input fingerprint, then reuses the verified static topology on later matching runs without caching model outputs;
- bounded researcher loop modules that search, select a follow-up query, conditionally repeat, and compress findings;
- richer per-topic research notes with key findings, evidence rows, implications, gaps, confidence, and source ids, so final synthesis has more structure than a short `summary`;
- richer final report contract with title, executive summary, key findings, comparison matrix, recommendations, limitations, open questions, and cited source ids;
- AIR state-machine `append` actions for typed state arrays, so repeated search results are accumulated as structured evidence before compression instead of being overwritten by the latest tool call;
- local search tool normalization with raw-content deduplication and configurable `max_results`, so repeated documents do not consume downstream compression context;
- artifact provenance in the native AIR VM: tool outputs can register `artifacts[]` or `documents[]`, and later model/return outputs with `sources`, `citations`, or `source_ids` must reference known artifact ids when evidence is available;
- expanded local deep-research corpus for the EV readiness example, covering 800V platforms, SiC supply chain, charging infrastructure, solid-state battery risks, distributed-drive controls, and comparative 2026-2030 readiness;
- native HTTP JSON tool provider for production-shaped tool adapters, with capability checks, templated request body/headers, JSON response parsing, and smoke coverage;
- AIR input `truncate` expressions, so bounded modules can cap large accumulated evidence before model calls without hiding the truncation in prompt text;
- explicit researcher reflection via the `research.think` tool, so deliberation appears in trace as a real AIR tool call instead of being hidden inside prompt text;
- runtime `retry.max_attempts` semantics for model and tool actions, including schema-error feedback through `_air_retry` so real model responses can be corrected and revalidated without weakening AIR schemas;
- provider token-limit retry policy for model calls, where provider context-length errors can retry with a declared `token_limit.max_input_chars` budget and `_air_retry.reason = "token_limit"` instead of relying on prompt-only fallback;
- AIR state-machine and RunPlan node conditions with `==`, `!=`, `&&`, and `||`, enabling rules such as `phase == "route" && research_direction.complete == false` and optional node routes such as `when: supervisor.decision.action == "conduct_research" || supervisor.decision.needs_more == true`;
- RunPlan-level `requires.capabilities` admission control, where every static node and dynamic fan-out module must be covered by the plan capability boundary before execution proceeds;
- runtime enforcement of `policy.max_tool_calls` and `policy.max_model_calls`, so bounded researcher loops fail closed if they try to exceed their declared tool-call or model-call budget;
- runtime tool capability handshake, where provider-configured tool capabilities must match module `tools[].capability` and be present in `requires.capabilities` before the AIR VM runs the tool;
- native VM approval provider hook for `kind: approval`, where the default provider fails closed, explicit providers or `tool-config.approvals` can approve or deny requested capabilities, and approval decisions are emitted in trace;
- runtime enforcement of action `timeout_seconds` and module-level `policy.timeout_seconds`, with action deadlines forwarded to timeout-aware providers such as OpenAI-compatible model calls and HTTP JSON tools;
- explicit supervisor reflection through `research.think` before conditional second-wave routing;
- typed supervisor routing via `action: conduct_research | research_complete` and bounded `follow_up_topics`, so the second wave is delegated by structured AIR state instead of prompt convention;
- bounded multi-step supervisor routing via `deep-research-supervised-two-step.air-plan.yaml`, where repeated supervisor decisions can run or skip later researcher waves without runtime graph mutation;
- nested connection paths, including `plan.plan.research_brief`;
- array index paths, including `plan.plan.topics[0]`;
- array append fan-in, including `topic_1.note -> final.notes[]`;
- AIR schema support for array `min_items` and `max_items`;
- runtime validation of array item bounds in the native AIR VM;
- real model execution through an OpenAI-compatible BigModel config.
- real dynamic fan-out execution through the OpenAI-compatible BigModel config, where one planning model call materializes four researcher instances and then synthesizes a final report.

## Validation

Run these checks before claiming this migration path works:

```bash
examples/deep-research/dev/verify.sh
```

The script covers formatting, the full Rust test suite, every deep-research module, every checked deep-research RunPlan, linker AIR VM tests, and local search deduplication. To include real model planning and execution, run:

```bash
AIR_DEEP_RESEARCH_REAL=1 examples/deep-research/dev/verify.sh
```

The expanded manual commands below are useful when debugging individual checks:

```bash
cargo fmt --check
cargo test
target/debug/air dev make-plan \
  --task "Compare the commercial readiness of 800V EV platforms, silicon carbide drives, solid-state batteries, and distributed drive systems for automakers planning 2026-2030 products. Build a deep research workflow with a planning step, multiple bounded researcher steps, and a final report." \
  --store examples/deep-research/module-store.air-store.yaml \
  --model-config examples/bigmodel-openai-compatible.json \
  --allow-internal \
  --output target/generated/deep_research_planned_current.air-plan.yaml
target/debug/air dev validate-plan target/generated/deep_research_planned_current.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml
```

The recipe-selection path has also been verified with:

```bash
target/debug/air dev make-plan \
  --task "Compare the commercial readiness of 800V EV platforms, silicon carbide drives, solid-state batteries, and distributed drive systems for automakers planning 2026-2030 products. Build a deep research workflow with a planning step, multiple bounded researcher steps, and a final report." \
  --store examples/deep-research/module-store.air-store.yaml \
  --model-config examples/bigmodel-openai-compatible.json \
  --allow-internal \
  --output target/generated/deep_research_planned_recipe_selection.air-plan.yaml
target/debug/air dev validate-plan target/generated/deep_research_planned_recipe_selection.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml
```

For AIR-only real execution, run the planner-generated RunPlan through the AIR VM with `examples/bigmodel-openai-compatible.json` and `examples/deep-research/tools.json`:

```bash
target/debug/air dev run-plan target/generated/deep_research_planned_current.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --input examples/deep-research/input.json \
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config examples/deep-research/tools.json \
  --checkpoint-out target/generated/deep_research_planned_current.state.json \
  --trace-out target/generated/deep_research_planned_current.trace.jsonl \
  > target/generated/deep_research_planned_current.output.json
```

The checked deep-research example can also be run through the generic profile path, which keeps CLI complexity out of app-specific commands:

```bash
target/debug/air dev validate-plan --profile examples/deep-research/profile.air-profile.yaml
target/debug/air dev run-plan --profile examples/deep-research/profile.air-profile.yaml
target/debug/air dev run-plan --profile examples/deep-research/profile.air-profile.yaml --parallel
```

The trace-specialization prototype is exposed through the hidden developer `replay` command so it does not expand the public command surface:

```bash
target/debug/air dev replay target/generated/parallel_smoke.trace.jsonl \
  --specialize-run-plan \
  --store tests/plans/parallel-smoke.air-store.yaml \
  --output target/generated/parallel_smoke.specialized.air-plan.yaml \
  --identity-out target/generated/parallel_smoke.identity.json
target/debug/air dev validate-plan target/generated/parallel_smoke.specialized.air-plan.yaml \
  --store tests/plans/parallel-smoke.air-store.yaml
```

For AIR-only clarification/resume validation, run the clarified plan with `--state-out` or `--checkpoint-out`, then continue with `resume-plan --profile examples/deep-research/profile.air-profile.yaml --state ... --override clarify.clarification.normalized_question=...`. This exercises AIR linker persistence and resume without relying on a generated backend.

An AIR-only real-model smoke run has been verified with BigModel/OpenAI-compatible chat completions and `examples/deep-research/tools.json`:

```bash
target/debug/air dev run-plan examples/deep-research/deep-research-clarified.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --input examples/deep-research/input.json \
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config examples/deep-research/tools.json \
  --log
```

The run exercised real model calls for clarification, planning, per-topic refinement, compression, and final reporting; local configured tools for search and reflection; schema retry after model field-name mistakes; and final report generation on the AIR VM.

The planner-generated AIR-only run has also been verified end to end. Its trace contained planner decisions, one planning model call, four researcher returns, 24 researcher tool calls, schema retry/error events followed by successful corrected model calls, and one final reporter call.

The supervised AIR-only run has also been verified with real model calls:

```bash
target/debug/air dev run-plan examples/deep-research/deep-research-supervised.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --input examples/deep-research/input.json \
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config examples/deep-research/tools.json \
  --trace-out target/generated/deep_research_supervised_real.trace.jsonl \
  > target/generated/deep_research_supervised_real.output.json
```

In that run, the supervisor emitted `action: conduct_research`, `needs_more: true`, and two bounded `follow_up_topics`; the second-wave researcher modules used those delegated topics as their actual `topic` inputs before final report synthesis.

For AIR-only bounded multi-step supervisor validation, run:

```bash
target/debug/air dev validate-plan examples/deep-research/deep-research-supervised-two-step.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml
cargo test -p air-linker two_step_supervised_deep_research
```

Those tests exercise two repeated supervisor decisions: one path stops after the second wave with `ResearchComplete`, and another path conducts a third bounded researcher wave from the second supervisor's delegated topics.

## Known Gaps

This is a credible bounded migration of the workflow shape, not yet a full production replacement for `open_deep_research`.

Missing or intentionally simplified:

- arbitrary dynamic fan-out-after-fan-out is limited to the declared parent-fanout form; ad hoc child graph mutation remains unsupported;
- unbounded supervisor ReAct loops beyond the current statically bounded one-step and two-step supervisor examples;
- richer condition expressions beyond equality, inequality, conjunction, and disjunction;
- preemptive cancellation for arbitrary synchronous providers; timeout-aware HTTP/model providers can enforce request deadlines, but AIR does not yet isolate and terminate any blocking provider implementation;
- provider-native search behavior for OpenAI and Anthropic web search beyond the generic HTTP JSON adapter;
- Tavily/API-native summarization and provider-specific search ranking beyond generic HTTP JSON and local search normalization;
- MCP server auth and tool discovery;
- report-quality parity with `open_deep_research` remains unproven; AIR has a deep-research report quality evaluator and real BigModel samples that pass it, but it does not yet benchmark output quality against the upstream app or Deep Research Bench;
- semantic summarization fallback for token-limit recovery beyond the current deterministic char-budget input compaction;
- intra-module checkpointing and continuation for arbitrary long-running in-flight modules;
- making parallel execution the default for `run-plan`; current true parallel execution is available through `run-plan --parallel`, while the default public CLI path remains sequential for compatibility;
- evaluator parity with the source repository's benchmark scripts.

## Migration Bar

For the current AIR-only slice, AIR should be considered to have migrated this app's core workflow when:

- the user-facing path is `air dev make-plan` plus `air dev run-plan`, not hand-authored topic topology;
- the generated plan validates against module input/output schemas;
- the generated plan runs on the AIR VM;
- tool calls and model calls are real, not mocked, for at least one non-trivial research query;
- the missing LangGraph dynamic features above are either implemented as explicit AIR primitives or documented as unsupported.

Current report-quality evidence:

- `examples/deep-research/dev/verify.sh` checks the richer final-report prompt, richer researcher note schema, expanded local corpus, and a 900+ word report quality sample through `examples/deep-research/dev/evaluate_report.py`;
- with `AIR_DEEP_RESEARCH_REAL=1`, the same verifier runs a real BigModel final-reporter smoke and evaluates the output with the same quality check;
- `target/generated/deep_research_dynamic_real.output.json` is a local real dynamic AIR VM sample from the OpenAI-compatible BigModel provider; it passed the quality evaluator with 1313 report words, 10 key findings, 6 comparison rows, 7 recommendations, 8 sources, and 12 limitations.
- the checked-in upstream `open_deep_research/examples/*.md` examples are roughly 1257-1680 words, so the current AIR real dynamic sample is in the same report-length band rather than the earlier short-summary shape.

Generated backend portability is intentionally out of scope for the current AIR-only slice.
