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

# Inspect which local skills match a task without spending model calls.
cargo run -p air-cli -- skill route "use TDD to refactor the parser"

# Run a task through AIR's entry agent.
# AIR routes skills and picks the right executor. Small coding tasks use code-agent;
# larger project tasks can be routed to the project orchestrator.
cargo run -p air-cli -- run "use TDD to fix the failing add function" --log
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
cargo run -p air-cli -- skill route "fix a security vulnerability" --explain
```

### Skill composition

Multiple compatible instruction skills stack onto one executor:

```bash
# User entry point: auto-route skills and run through the host executor
cargo run -p air-cli -- run "use TDD to fix the failing add function"

# Explain executor, routed skills, permissions, and verification without running
cargo run -p air-cli -- run "use TDD to fix the failing add function" --explain

# Force additional instruction skills
cargo run -p air-cli -- run "refactor a helper and run tests" \
  --skills tdd-workflow

```

Skills declare `incompatible_with` to prevent contradictory instructions from loading together.

### Skill lifecycle

```bash
cargo run -p air-cli -- skill list
cargo run -p air-cli -- skill validate code-agent
cargo run -p air-cli -- skill explain code-agent
cargo run -p air-cli -- skill audit code-agent
cargo run -p air-cli -- skill import ./some-skill
cargo run -p air-cli -- skill upgrade tdd-workflow --dry-run
```

Audit risk gates (`high`/`critical`) refuse to load or run untrusted skills.

External skills are imported as local AIR packages before use. Import does not execute install scripts:

```bash
cargo run -p air-cli -- skill import \
  https://github.com/affaan-m/everything-claude-code/tree/main/skills/tdd-workflow

# Collection repositories are split into one AIR instruction skill per SKILL.md.
cargo run -p air-cli -- skill import https://github.com/anthropics/skills
```

AIR records imported skill provenance in `source.json` and trust decisions in
the repo-level `skills.lock`. The lock pins source, content hash, audit hash,
and the local trusted/untrusted decision; skill manifests and source metadata
describe provenance but do not grant trust by themselves. Imported scripts are
not executed automatically; they remain skill assets unless wrapped by an AIR
tool.

```bash
cargo run -p air-cli -- skill audit tdd-workflow
cargo run -p air-cli -- skill route "use TDD to refactor a Rust helper"
```

## MCP Governance

AIR can call Streamable HTTP MCP servers through configured tools, but MCP is
treated as governed network capability rather than an untracked side channel.
Inspect MCP declarations before exposing them to an agent:

```bash
cargo run -p air-cli -- mcp list --tool-config skills/code-agent/tools.json
cargo run -p air-cli -- mcp explain github.issue --tool-config tools.json
cargo run -p air-cli -- mcp audit --tool-config tools.json
```

Each `kind: "mcp"` tool declares a `server_url`, optional fixed MCP `tool`,
auth headers or `bearer_token_env`, and a narrow AIR `capability` such as
`github.issue.read`. MCP tool calls emit provenance artifacts with server URL,
method, and selected MCP tool.

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
| Context window is bounded | Observations are compacted before model calls; file reads require bounded selectors |
| Repeated context searches can be gated | Runtime `repeated_tool_policy` can require `repeat_reason` for grep/LSP/symbol lookups |

### Artifacts and replay

Code-agent runs can write an auditable artifact directory and replay it later without spending model calls:

```bash
cargo run -p air-cli -- run "refactor a helper and run tests" \
  --trace-out target/generated/code-runs/helper.trace.jsonl \
  --artifact-out target/generated/code-runs/helper \
  --log

cargo run -p air-cli -- run "refactor a helper and run tests" \
  --replay-artifact target/generated/code-runs/helper
```

Use `--replay-from <trace-line>` to replay the earlier trace up to a chosen event and switch back to live execution from that point.

## Project Workflows

For tasks larger than one edit loop, AIR has a project orchestrator with task DAGs, per-task worktree isolation, skill assignment, and diff constraints:

```bash
# The simple entry point can route project-sized work and write a reviewable manifest.
cargo run -p air-cli -- run "refactor the tools crate into smaller modules" --mode project

# Advanced: review and run an explicit project manifest.
cargo run -p air-cli -- project plan "refactor the tools crate into smaller modules" \
  --output air-project.yaml
cargo run -p air-cli -- project run --file air-project.yaml --log
cargo run -p air-cli -- project status --file air-project.yaml
cargo run -p air-cli -- project verify --file air-project.yaml
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

## Improve Loop

AIR can mine existing run artifacts and benchmark outputs for failed runs, group
them into findings, and suggest regression fixtures before any self-improvement
patch is attempted:

```bash
cargo run -p air-cli -- improve

cargo run -p air-cli -- improve next
cargo run -p air-cli -- improve check IMP-001
cargo run -p air-cli -- improve promote IMP-001
cargo run -p air-cli -- regression run IMP-001
cargo run -p air-cli -- improve evaluate IMP-001

cargo run -p air-cli -- improve \
  --from target/generated/code-agent-bench/<run-id> \
  --write-regressions
```

The default output is `.air/improve/latest/` with `observations.json`,
`findings.json`, `suggested_regressions.json`, and `report.md`. This command is
read-only with respect to AIR source code; it only turns real failures into
evidence that can be promoted into benchmarks.

Promoted regressions become executable gates through `air regression run`.
`air improve evaluate` runs the promoted regression, an improve-focused test
gate, and deterministic anti-reward-hacking checks before returning
`accept_candidate`, `reject_candidate`, or `needs_review`.

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

## Low-Level AIR Module (Advanced)

Most users should start from skills. AIR modules are the lower-level typed IR that executor skills and advanced workflows compile to or run directly:

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
cargo run -p air-cli -- dev validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
cargo run -p air-cli -- dev run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

## Deep Research

Deep research demonstrates multi-agent workflows with clarification, dynamic fan-out/fan-in, nested fan-out, parallel execution, checkpoint/resume, and JIT hot-path specialization:

```bash
cargo run -p air-cli -- dev validate-plan --profile examples/deep-research/profile.air-profile.yaml
cargo run -p air-cli -- dev run-plan --profile examples/deep-research/profile.air-profile.yaml --parallel
```

## CLI Surface

| Command | Description |
| --- | --- |
| `run` | User entry point; routes skills and picks code-agent or project-agent |
| `skill route` | Route a task to matching skills |
| `skill list/validate/explain/audit/import/upgrade` | Skill lifecycle management |
| `mcp list/explain/audit` | Inspect MCP tool governance before runs |
| `bench code` | Benchmark code agent on a suite |
| `bench skill` | Benchmark with skill preload, optional no-skill comparison |
| `improve` | Mine artifacts/bench runs for failures and suggested regressions |
| `regression run` | Execute promoted AIR regression candidates |
| `dev` | Advanced IR/runtime tools for AIR development |

Lower-level IR commands live under `air dev` (`dev validate-plan`, `dev make-plan`, `dev run-plan`, `dev resume-plan`, `dev replay`, and `dev run-module`). The public workflow should start from `air run`.

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
- [ ] **Skill registry and governance.** A publish/install system for sharing audited AIR skill packages with capability, route, benchmark, and provenance metadata.
- [ ] **Schema conformance testing.** A lightweight test harness that calls real LLMs but only validates output structure against the declared AIR schema — no assertion on specific content.
- [ ] **Multi-executor skill routing.** Support multiple executor skills (not just `code-agent`) so the router can choose between entirely different agent architectures.
- [ ] **Skill marketplace metrics.** Aggregate benchmark results across skills to surface which instruction sets actually improve task success rates.
