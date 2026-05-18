# AIR

**Bounded code agents with auditable traces, composable skills, and runtime-enforced safety.**

AIR is a Rust runtime for coding agents that optimizes for **what you can verify after the agent runs**, not for how much freedom the agent has during execution. Agents are declarative state machines in `.air.yaml`; the VM enforces tool constraints, verification gates, and context budgets; traces record everything that happened.

## Why AIR

The core insight: **context window is scarce, model self-assessment is unreliable, and agent runs must be auditable**.

| Problem | AIR approach |
| --- | --- |
| Model says "done" without verifying | `CodeRunVerdict` derives pass/fail from trace events, not model output |
| Agent reads entire files, blows context | `read_range` / `read_contains` — bounded reads only, no unscoped access |
| Verification commands silently edit files | Workspace snapshots detect file changes; verification that mutates workspace does not count as passed |
| One agent must handle everything | Skill routing picks the right instruction set per task; skills compose on one executor |
| Tool permissions are hints to the model | `allowed_tools` on `tool_batch_dispatch` is enforced at runtime, not in the prompt |
| Long runs are opaque | JSONL traces, code-run artifacts, sub-agent trace/output, markdown bench reports |

## Status

AIR is a local compiler/runtime prototype. The native Rust VM is the reference runtime.

| Feature | Status |
| --- | --- |
| Typed state-machine AIR modules | Supported |
| Bounded tool dispatch with runtime-enforced `allowed_tools` | Supported |
| `CodeRunVerdict` — trace-derived pass/fail/verification/patch/constraints | Supported |
| Skill routing, composition, and instruction/executor separation | Supported |
| Workspace change detection (bash/command snapshots) | Supported |
| Project orchestrator with per-task DAG, worktree isolation, skills, constraints | Supported |
| Sub-agent observability (`.air/subagents/` trace + output) | Supported |
| Benchmark suite with `--report` markdown output and skill comparison | Supported |
| `_air` runtime namespace protection | Supported |
| RunPlan module composition, dynamic fan-out/fan-in, checkpoint/resume | Supported |
| Trace redaction and sensitive-field policy | Supported |

## Quick Start

```bash
cargo build
cargo run -p air-cli -- validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

`air` auto-loads a repository-root `.env`:

```dotenv
AIR_MODEL_PROFILE=glm

AIR_MODEL_GLM_API_KEY=...
AIR_MODEL_GLM_BASE_URL=https://open.bigmodel.cn/api/coding/paas/v4
AIR_MODEL_GLM_MODEL=GLM-5.1
```

Switch providers by changing `AIR_MODEL_PROFILE`. To use `OPENAI_*` directly, leave it unset.

## Skills

Skills separate **instructions** (what the agent should know) from **execution** (how it runs). The built-in `code-agent` is the executor; instruction skills like `tdd-workflow` inject task-specific guidance.

### Skill routing

The router matches a natural-language task to the best instruction skills using triggers, description tokens, repo markers, and language/task-type hints:

```bash
# See which skills match a task
cargo run -p air-cli -- skill route "use TDD to refactor the parser"

# Explain the routing decision
cargo run -p air-cli -- skill explain-route "fix a security vulnerability"
```

### Skill composition

Multiple compatible instruction skills stack onto one executor:

```bash
# Auto-route and run
cargo run -p air-cli -- skill run --auto "use TDD to fix the failing add function"

# Explicit skill
cargo run -p air-cli -- skill run code-agent "refactor a helper and run tests"
```

Skills declare `incompatible_with` to prevent contradictory instructions from loading together.

### Skill lifecycle

```bash
cargo run -p air-cli -- skill list
cargo run -p air-cli -- skill validate code-agent
cargo run -p air-cli -- skill explain code-agent
cargo run -p air-cli -- skill audit code-agent
cargo run -p air-cli -- skill import ./some-skill --out skills/vendor/some-skill
cargo run -p air-cli -- skill compile code-agent
```

Audit risk gates (`high`/`critical`) refuse to load or run untrusted skills.

## Code Agent

The code agent edit loop is a phased state machine:

```
init → choose → act → post_act → verify → verify_act → ... → summarize → done
```

Key safety properties:

| Property | Mechanism |
| --- | --- |
| Agent cannot read whole files | `read_range` and `read_contains` require offset/limit or a search query |
| Verification is separate from editing | `verify` phase only gets `bash`; `allowed_tools` enforced at runtime |
| Verification that edits files does not count | Workspace snapshots before/after bash detect mutations |
| Model self-report is not trusted | `CodeRunVerdict` scans trace events for actual verification tool calls |
| Context window is bounded | 600KB observation window, 16KB unscoped read preview, compaction |
| Repeated reads/searches are blocked | Runtime `repeated_tool_policy` requires `repeat_reason` for context tools |

## Project Workflows

For tasks larger than one edit loop, AIR has a project orchestrator with task DAGs, per-task worktree isolation, skill assignment, and diff constraints:

```bash
cargo run -p air-cli -- project plan "refactor the tools crate into smaller modules" \
  --output air-project.yaml

cargo run -p air-cli -- project run --log
cargo run -p air-cli -- project status
cargo run -p air-cli -- project verify
```

Each task runs in an isolated workspace copy. On success, changes are merged back only if the main workspace snapshot still matches the task baseline. Project manifests support per-task `skills`, `allowed_files`, `forbidden_files`, `verification`, and `success_conditions`.

```yaml
schema: air.project.v1
project:
  name: air-project
  goal: refactor the tools crate into smaller modules
defaults:
  profile: skills/code-agent/edit.air-profile.yaml
  model_config: examples/bigmodel-openai-compatible.json
  tool_config: skills/code-agent/tools.json
  artifact_dir: .air/project
tasks:
  - id: split_file_tools
    goal: split file read/write/edit helpers into focused modules
    depends_on: []
    skills: ["tdd-workflow"]
    allowed_files: ["crates/air-tools/src/**"]
    forbidden_files: ["target/**"]
    verification:
      - command: cargo test -p air-tools
    success_conditions:
      required_changed_files: ["crates/air-tools/src/lib.rs"]
      required_diff_contains: ["mod file"]
    max_changed_files: 8
    max_diff_lines: 400
```

## Benchmarks

AIR includes a benchmark framework for measuring code agent quality:

```bash
# Run a benchmark suite
cargo run -p air-cli -- bench code --suite suite.json --log --report report.md

# Benchmark with a skill, comparing against no-skill baseline
cargo run -p air-cli -- bench skill tdd-workflow --suite suite.json --compare-no-skill --report report.md
```

Benchmarks produce JSON run artifacts and optional Markdown reports with per-task pass/fail, model calls, tool calls, and failure reasons. Code-run artifacts are cached by input fingerprint and replayed on subsequent runs.

## CodeRunVerdict

Every code agent run produces a `CodeRunVerdict` — a structured pass/fail derived from trace events:

| Field | Source |
| --- | --- |
| `patch_applied` | Workspace snapshot diff (did files change?) |
| `verification_ran` | Trace scan for verification tool calls |
| `verification_passed` | Verification tool returned success without mutating workspace |
| `allowed_files_ok` | Changed files within allowed set |
| `required_files_ok` | Required files were actually changed |
| `forbidden_files_ok` | No forbidden files changed |
| `required_diff_ok` | Diff contains required text |
| `max_diff_lines_ok` | Diff within line budget |
| `final_success` | All of the above |

This replaces trusting the model's self-reported `final_success`. If the trace shows no verification ran, the verdict fails even if the model claims success.

## Sub-agents

The `task` tool launches isolated exploration sub-agents. Each sub-agent writes its own trace, output, and artifacts under `.air/subagents/task-{timestamp}/`. The parent trace records child paths and metrics (model calls, tool calls, errors) without embedding the full child output, keeping the parent context compact.

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

```bash
cargo run -p air-cli -- validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

## Deep Research

Deep research demonstrates multi-agent workflows with clarification, dynamic fan-out/fan-in, nested fan-out, parallel execution, checkpoint/resume, and JIT hot-path specialization:

```bash
cargo run -p air-cli -- validate-plan --profile examples/deep-research/profile.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/deep-research/profile.air-profile.yaml --parallel
```

## CLI Surface

| Command | Description |
| --- | --- |
| `code` | Run the code agent edit loop on a task |
| `skill run` | Run a skill explicitly or with `--auto` routing |
| `skill route` | Route a task to matching skills |
| `skill list/validate/explain/audit` | Skill lifecycle management |
| `project plan/run/status/verify` | Multi-task project orchestrator |
| `bench code` | Benchmark code agent on a suite |
| `bench skill` | Benchmark with skill preload, optional no-skill comparison |
| `run-plan` | Execute a RunPlan or packaged profile |
| `resume-plan` | Resume a halted/checkpointed plan |

## Verification

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Documentation

- [AIR User Guide](docs/user_guide.md)
- [Condition DSL](docs/condition_dsl.md)
- [Open Deep Research Migration Notes](docs/open_deep_research_migration.md)

## Roadmap

- [ ] **Whole-program compilation (merge + flatten).** Flatten a multi-module plan into a single state machine with unified state schema, resolved field names, and merged policies. Analogous to LLVM LTO: separate compilation for development, whole-program compilation for output.
- [ ] **Module registry.** A publish/install system for sharing agent modules across projects. Teams publish versioned modules and consume them via dependency declarations.
- [ ] **Schema conformance testing.** A lightweight test harness that calls real LLMs but only validates output structure against the declared AIR schema — no assertion on specific content.
- [ ] **Multi-executor skill routing.** Support multiple executor skills (not just `code-agent`) so the router can choose between entirely different agent architectures.
- [ ] **Skill marketplace metrics.** Aggregate benchmark results across skills to surface which instruction sets actually improve task success rates.
