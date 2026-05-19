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

# Run a task through AIR's entry agent.
cargo run -p air-cli -- run "use TDD to fix the failing add function" --log

# Explain the selected executor, skills, memory, and permissions without running a model.
cargo run -p air-cli -- run "use TDD to fix the failing add function" --explain
```

`air` defaults to the local OpenAI-compatible model config at
`examples/local-openai-compatible.json` and auto-loads a repository-root `.env`:

```dotenv
AIR_MODEL_LOCAL_API_KEY=...
AIR_MODEL_LOCAL_BASE_URL=http://localhost:11434/v1
AIR_MODEL_LOCAL_MODEL=qwen2.5-coder
```

Use `--model-config` to point a command at another OpenAI-compatible provider config.

AIR also reads optional local defaults from `.air/config.yaml`:

```yaml
model_config: .air/models/local.json
memory_dir: .air/memory
dream_dir: .air/dream
```

Per-command flags still win over config values.

## Product Surface

AIR has one primary public entry point and flat top-level commands for the
parts teams actually inspect:

| Layer | Commands | Audience |
| --- | --- | --- |
| Daily | `air run`, `air status`, `air brain` | Developers and platform teams checking current state |
| Governance | `air skill`, `air dream`, `air findings`, `air memory`, `air mcp`, `air bench`, `air audit` | Teams governing skills, tools, evals, and Dream review |
| Experimental | `air improve`, `air self`, `air eval`, `air project`, `air regression`, `air dev` | Controlled self-improvement experiments and AIR runtime development |

The default workflow is intentionally narrow: run a bounded coding task, check
`air status`, then inspect what the agent has learned with `air brain`.

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

### Deterministic audit

Audit a code-run artifact without calling a model:

```bash
cargo run -p air-cli -- audit run target/generated/code-runs/helper \
  --report target/generated/code-runs/helper-audit.md
```

`air audit run` emits an `air.audit.v1` JSON report with deterministic sections
for correctness, safety, verification, tool use, context, cost, and
reward-hacking risk. The final audit verdict is derived from artifact files,
workspace diff, `CodeRunVerdict`, and trace events; LLM advisory explanations
can be layered on later, but they do not decide pass/fail.

Summarize a window of runs for platform review:

```bash
cargo run -p air-cli -- audit collect \
  --from target/generated \
  --since-unix 1770000000 \
  --limit 100 \
  --out-dir .air/audit/latest \
  --report .air/audit/latest.md
```

`air audit collect` scans for code-run artifacts, writes one audit JSON per run,
and produces an `air.audit_collection.v1` summary with pass/fail counts, finding
categories, severity counts, skill/tool distribution, top cost runs, high-risk
runs, scan-window metadata, and collection errors. Use `--since-unix` and
`--limit` to keep periodic platform reviews incremental instead of re-scanning a
large `target/generated` tree. Teams can run it daily, weekly, after a release,
or over any artifact directory they want to review.

### Dream: offline optimization review

Dream packages AIR's audit-first improvement loop as a periodic offline
experience compiler: agents work during the day, then AIR reviews the traces and
artifacts later. Dream does not trust model self-assessment and does not change
source code by itself. It runs deterministic audit collection, mines findings
and suggested regressions, compiles evidence-backed memory candidates, writes a
Dream report, advances evidence-gated memory, and gives the platform team
reviewable next commands.

```bash
cargo run -p air-cli -- dream run \
  --from target/generated \
  --limit 100 \
  --mode deep \
  --advance-timeout-seconds 600 \
  --write-regressions
```

`dream run` is incremental by default. It reads `.air/dream/state.json`, uses
per-root high-watermark cursors minus a small overlap as the next scan window,
and de-duplicates exact inputs already seen in the prior window. Use `--full`
to ignore the saved state, or pass `--since-unix` to set an explicit window. By
default, runs are written under `.air/dream/runs/<run-id>/` and
`.air/dream/latest` points at the newest run; pass `--out-dir` for a custom
destination. The audit and improve stages are both derived from the same
`window/window.json` manifest, so the report does not mix full-history audit
with windowed improvement findings.

Dream has three depths:

- `--mode micro`: short consolidation. It selects the window and records
  episode memory without running audit/improve mining or Dream IR.
- `--mode deep`: the default. It runs full audit + improve + memory synthesis
  and writes Dream IR concept, hypothesis, and policy candidates.
- `--mode evolution`: deep mode plus conservative cross-domain proposals. Add
  `--experiment --write-regressions` to run bounded self-fix experiments for
  top findings; experiments still do not promote patches or open PRs.

```bash
cargo run -p air-cli -- dream state
cargo run -p air-cli -- dream run --full --from target/generated
```

The output schema is `air.dream.v1`; the state schema is `air.dream_state.v1`.
The Dream directory contains:

- `dream.json` and `dream.md`: executive summary and next commands.
- `window/window.json`: the exact artifact/run inputs selected for this Dream
  window, including path, kind, mtime, and fingerprint.
- `audit/`: deterministic audit collection and per-run audit reports.
- `improve/`: observations, findings, suggested regressions, and report.
- `memory/`: Dream memory extraction summary, Dream IR, and synthesis report
  for this window.
- `memory-advance/`: low-risk memory lifecycle updates, routing scorecards,
  validated skill-draft gates, and reviewed runtime-guard proposals.
- `logs/`: captured stdout/stderr from the underlying audit and improve stages.
- `.air/dream/state.json`: the persistent incremental cursor and last Dream
  summary, including the last window's input fingerprints and per-root cursors.
- `.air/dream/findings.jsonl`: append-only persistent finding records with
  stable finding keys, current display ids such as `IMP-001`, first_seen,
  last_seen, recurrence_count, status, linked regressions, linked candidates,
  and recurred_after_fix. `IMP-*` is a per-run display rank, not the durable
  identity.
- `.air/dream/ledger.jsonl`: append-only lifecycle events such as
  finding_observed, finding_resolved, and experiment_ran.
- `.air/memory/`: local long-term evidence store with `episodes.jsonl`,
  `graph.jsonl`, `ledger.jsonl`, `usage.jsonl`, and candidate cards under
  `cards/{failure,procedure,routing,concept,hypothesis,policy}`.

Dream memory is deliberately conservative. It writes episode records and
candidate failure/procedure/routing memories with source evidence, confidence,
impact, lifecycle status, and promotion gates. Dream also writes
`memory/dream_ir.json`, a deterministic synthesis IR whose concept, hypothesis,
and runtime-policy candidates are compiled into memory cards and graph edges.
Candidate memory is not injected into prompts, used for routing, compiled into
skills, or promoted into runtime policy until later validation gates approve it.
`dream run` now runs `memory advance` by default: repeated negative scorecard
evidence can retire memory, confirmed positive evidence can validate memory,
causal evidence can promote memory into the compact runtime pack, procedure
memory can be validated as an untrusted skill draft, routing outcomes are
measured, and policy memory is written as a guard-review proposal. It still
does not pin memory, import/trust generated skills, edit runtime guard code,
commit, or open PRs. Pass `--no-advance` for an observation-only Dream run.
Micro mode skips advance automatically. `--advance-limit` caps how many memory
cards the pass can touch, and `--advance-timeout-seconds` gives the advance
stage a wall-clock budget. `air run --timeout-seconds <n>` provides the same
kind of outer budget for foreground runs.

```bash
cargo run -p air-cli -- memory list --status candidate
cargo run -p air-cli -- memory search verification --kind failure
cargo run -p air-cli -- memory view mem_failure_... --evidence
cargo run -p air-cli -- memory promote mem_procedure_... --status validated
cargo run -p air-cli -- memory pack "fix a Rust verification failure"
cargo run -p air-cli -- memory graph --limit 20
cargo run -p air-cli -- memory scorecard
cargo run -p air-cli -- memory advance --timeout-seconds 600
cargo run -p air-cli -- memory causal-eval mem_procedure_... \
  --outcome helped \
  --evidence target/generated/compare-no-memory.json
cargo run -p air-cli -- memory promote mem_procedure_...
cargo run -p air-cli -- memory skill-draft mem_procedure_...
cargo run -p air-cli -- memory skill-evaluate mem_procedure_...
cargo run -p air-cli -- memory policy-check mem_policy_...
cargo run -p air-cli -- dream findings list --status open
cargo run -p air-cli -- dream findings resolve IMP-001 --fixed-by cand-1
```

Use `brain` when a platform team needs one organized view of what the agent has
learned and compiled:

```bash
cargo run -p air-cli -- brain
cargo run -p air-cli -- brain memory
cargo run -p air-cli -- brain skills
cargo run -p air-cli -- brain policies
cargo run -p air-cli -- brain findings
cargo run -p air-cli -- brain candidates
cargo run -p air-cli -- brain view mem_procedure_...
cargo run -p air-cli -- brain report --out .air/brain/report.md
```

`brain` is read-only. It aggregates `.air/memory/cards`, memory scorecards,
Dream findings, Dream experiment candidates, validated skill drafts, installed
skills, and guard proposals into a lifecycle-oriented view, so users can see
what is promoted, what is only candidate evidence, what has been compiled into a
skill draft, which candidate patches were tried, and what still needs benchmark
or human review. The default output is human-readable; add `--json` when
scripts or dashboards need the structured form.

The safe follow-up is still explicit: promote/review regressions, run
`air self fix ... --evaluate` only when a finding is worth fixing, compare
candidates, and open a PR manually. Dream can run that experiment chain only
when explicitly requested:

```bash
cargo run -p air-cli -- dream run \
  --mode evolution \
  --from target/generated \
  --write-regressions \
  --experiment \
  --top-findings 2 \
  --budget-seconds 900
```

The experiment writes candidates under the run directory's `candidates/`, logs
under `logs/`, and a `dream_provenance.json` file with `dream_run_id`,
`finding_stable_key`, and `window_manifest_sha`. Experiments require a clean git
workspace and currently run only executable `code_run_verdict` regressions;
unsupported regression kinds are skipped before model calls. It still does not
trust, promote patches, commit, or open a PR. Memory follow-up is default-closed
loop but human-gated at the dangerous boundary: `air run` includes promoted
Dream memory by default and accepts `--no-memory` to disable it; Dream records
which memories were used in the code-run artifact. If no `--artifact-out` is
supplied, AIR writes one under `target/generated/code-runs/memory-run-*` so the
next Dream pass can turn successful runs into `helped_candidate` events and
failed runs into `hurt_candidate` events, visible through `air memory
scorecard`. Repeated positive evidence confirms `helped` and validates
candidate memory. Causal evidence recorded with `air memory causal-eval`,
normally from compare-no-memory, replay, or benchmark output, lets `memory
advance` promote memory into future packs. Repeated negative evidence confirms
`hurt` and auto-retires the memory so it stops being suggested. Procedure
memory can become a validated skill draft, but import/trust still requires
manual review. Policy memories become deterministic-guard proposals under
`memory-advance/guards/`; accepted guards are implemented as normal reviewed
patches. If Dream was run without `--write-regressions`, its next
commands first promote the selected regression and then point `self fix` at the
promoted regression file under `skills/code-agent/benches/regressions/`.
Deterministic gates remain the source of truth for promotion.

`air run` emits a single stable JSON envelope for every executor:

```json
{
  "schema": "air.run.v1",
  "task": "...",
  "requested_mode": "auto",
  "executor": "code-agent | review-agent | bench-agent | project-agent",
  "memory": { "enabled": true, "cards": 3 },
  "output": {}
}
```

Code, review, bench, and project results all live under `output`, so CI,
dashboards, Dream ingest, and platform tools can parse one top-level shape
instead of special-casing executor output.

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
  model_config: examples/local-openai-compatible.json
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

## Eval Integrity

Pin the evaluation corpus before using self-improvement candidates:

```bash
cargo run -p air-cli -- eval manifest \
  --out .air/evals/manifest.json

cargo run -p air-cli -- eval check \
  --manifest .air/evals/manifest.json
```

The manifest stores sha256 hashes for benchmark suites, promoted regressions,
and scoring/evaluation code. Its protected paths are built from broad eval/trust
globs plus the actual pinned git-tracked integrity files, so AIR does not depend
on one hard-coded runner path. `air improve evaluate` automatically runs this
check when `.air/evals/manifest.json` exists. A candidate that edits protected
eval files, changes the manifest, removes a pinned file, or changes a pinned
file's hash is blocked as `needs_review`.

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
cargo run -p air-cli -- self prepare IMP-001 --candidates 3
cargo run -p air-cli -- self fix IMP-001 --candidates 3 --evaluate
cargo run -p air-cli -- self fix IMP-001 --skill code-agent --candidates 3 --evaluate
cargo run -p air-cli -- self capture IMP-001 cand-1
cargo run -p air-cli -- self compare IMP-001 --out .air/candidates/IMP-001.md

cargo run -p air-cli -- improve \
  --from target/generated/code-agent-bench/<run-id> \
  --write-regressions
```

The default output is `.air/improve/latest/` with `observations.json`,
`findings.json`, `suggested_regressions.json`, and `report.md`. This command is
read-only with respect to AIR source code; it only turns real failures into
evidence that can be promoted into benchmarks. Findings are sorted by an
`impact_score` that combines frequency, task spread, changed-file spread, and
failure-category weight, so the report surfaces what is most worth fixing first.
`air self capture` stores a candidate's concrete workspace diff as
`patch.diff` plus `changed_files.json`, so the candidate scorecard points to the
actual code change being evaluated.
`air self fix` uses AIR's own `code-agent` in detached worktrees to generate
candidate patches; it can also evaluate each generated candidate before
comparison when `--evaluate` is passed. Each detached worktree is bootstrapped
with the caller's current tracked and untracked workspace overlay before the
candidate starts, then the captured patch is diffed only against that bootstrap
commit. The generated task prompt includes the selected finding, regression
fixture hints, benchmark task context, impact reason, and any rejected candidate
feedback from earlier candidates in the same run; each candidate slot also
records that prompt as `task.md`. Temporary worktrees are removed automatically
after AIR captures each candidate's patch, trace, artifact, and evaluation; pass
`--keep-worktrees` only when you need manual worktree inspection. Benchmark
suites, fixtures, regressions, eval
manifests, and `skills.lock` are evidence/gates, not acceptable candidate fix
targets.
Use `--skill <skill-id>` when the candidate should improve a local skill package
instead of AIR runtime code. External MCP servers remain out of scope; AIR can
improve the local skill/tool config/policy that governs how those MCP tools are
used.

Promoted regressions become executable gates through `air regression run`.
`air improve evaluate` runs the promoted regression, an improve-focused test
gate, eval-manifest integrity when present, and deterministic
anti-reward-hacking checks before returning `accept_candidate`,
`reject_candidate`, or `needs_review`.

The intended safe-improvement flow is:

```text
real run or bench failure
  -> deterministic audit
  -> finding
  -> promoted regression
  -> candidate patch in a reviewable candidate slot
  -> regression + tests + guard
  -> candidate scorecard
  -> PR, not direct promotion
```

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

| Layer | Command | Description |
| --- | --- | --- |
| Daily | `run` | User entry point for bounded coding tasks |
| Daily | `status` | One-screen workspace status: last Dream, open findings, memory, candidates |
| Daily | `brain memory/skills/policies/findings/candidates/view/report` | Organized view of memory, skill drafts, guard proposals, findings, and Dream experiment candidates |
| Governance | `audit run/collect` | Audit one code-run artifact or summarize many artifacts |
| Governance | `skill ...` | Skill routing, validation, audit, import, and upgrade |
| Governance | `dream ...` | Incremental offline audit, finding mining, and memory synthesis |
| Governance | `findings ...` | Shortcut for persistent Dream finding triage |
| Governance | `memory ...` | Evidence-backed memory lifecycle, scorecards, and skill/policy drafts |
| Governance | `mcp ...` | MCP tool governance before runs |
| Governance | `bench ...` | Code-agent and skill benchmark suites |
| Experimental | `improve ...` | Failure mining and suggested regressions |
| Experimental | `regression run ...` | Execute promoted AIR regression candidates |
| Experimental | `self ...` | Create, generate, capture, and compare candidate improvements |
| Experimental | `eval ...` | Pin and verify evaluation corpus integrity |
| Experimental | `project ...` | Bounded multi-task project orchestration |
| Experimental | `dev ...` | Advanced AIR IR/runtime tools |

Lower-level IR commands live under `air dev` (`dev validate-plan`,
`dev make-plan`, `dev run-plan`, `dev resume-plan`, `dev replay`, and
`dev run-module`). The public workflow should start from `air run`.

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
