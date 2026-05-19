# AIR User Guide

This guide is the shortest path from a clean checkout to running and modifying AIR workflows.

AIR has three layers:

- **Module**: a typed, bounded state machine in `.air.yaml`.
- **RunPlan**: a verified DAG that connects modules, including dynamic fan-out/fan-in.
- **Runtime**: the native AIR VM executes checked plans and writes auditable traces.

Host applications provide model, tool, and approval implementations through typed provider contracts.

## Company Adoption Flow

For a team or company, AIR is meant to be installed once as the governed agent
runtime around local skills, MCP tools, policy, evals, and run artifacts:

```bash
# 1. Connect company tools and inspect their capabilities.
cargo run -p air-cli -- mcp audit --tool-config tools.json

# 2. Import and audit company skills before trusting them.
cargo run -p air-cli -- skill import ./skills/company-rust-style
cargo run -p air-cli -- skill audit company-rust-style --write

# 3. Run developer tasks through the bounded entrypoint.
cargo run -p air-cli -- run "fix the failing invoice rounding test" \
  --artifact-out target/generated/code-runs/invoice-rounding

# 4. Audit the exact run artifact before using it as evidence.
cargo run -p air-cli -- audit run target/generated/code-runs/invoice-rounding \
  --report target/generated/code-runs/invoice-rounding-audit.md

# 5. Periodically summarize any window of runs for platform review.
cargo run -p air-cli -- audit collect \
  --from target/generated \
  --since-unix 1770000000 \
  --limit 100 \
  --out-dir .air/audit/latest \
  --report .air/audit/latest.md

# 6. Run Dream to package audit + improve mining into one offline review.
cargo run -p air-cli -- dream run \
  --from target/generated \
  --limit 100 \
  --out-dir .air/dream/latest \
  --write-regressions

cargo run -p air-cli -- dream state

# 7. Pin evaluation files before evaluating candidate improvements.
cargo run -p air-cli -- eval manifest --out .air/evals/manifest.json
cargo run -p air-cli -- eval check --manifest .air/evals/manifest.json

# 8. Mine real failures into regressions and gates.
cargo run -p air-cli -- improve --from target/generated --write-regressions
cargo run -p air-cli -- regression run --all
cargo run -p air-cli -- self prepare IMP-001 --candidates 3
cargo run -p air-cli -- self fix IMP-001 --candidates 3 --evaluate
cargo run -p air-cli -- self compare IMP-001 --out .air/candidates/IMP-001.md
```

Developers mostly use `air run`. Platform teams own `tools.json`, imported
skills, `skills.lock`, benchmark suites, promoted regressions, and audit policy.
This separation keeps model freedom behind deterministic evidence: every useful
run should leave a trace, artifact, verification result, and audit report that
can be reviewed without trusting the model's self-assessment.

The review window is operational, not hard-coded. A team can collect runs every
day, every release, every incident review, or after enough new artifacts have
accumulated. The collection report is the platform team's starting point: top
failure categories become findings, repeated findings become regressions, and
candidate runtime/skill/tool changes are evaluated against those gates before a
PR is opened.

`air dream run` is the productized version of that review window. It is
incremental by default: AIR reads `.air/dream/state.json`, uses the previous
successful Dream completion time as the next scan cursor, and updates the state
only after the Dream run succeeds. Use `--full` to ignore the saved cursor, or
`--since-unix` to force a specific window. It writes
`.air/dream/latest/dream.json` and `.air/dream/latest/dream.md`, plus `audit/`,
`improve/`, and `logs/` subdirectories. Dream is intentionally offline and
review-first: it audits recent artifacts, mines findings, optionally writes
suggested regression JSON files, and prints next commands. It does not modify
source code, open PRs, or trust generated patches without the normal
regression/test/guard/candidate comparison gates.

Once `.air/evals/manifest.json` exists, `air improve evaluate` checks it
automatically. This protects the loop from candidates that pass by editing
benchmark suites, promoted regressions, scoring code, or the manifest itself.

## 1. Run The Simple Example

```bash
cargo run -p air-cli -- dev validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml

cargo run -p air-cli -- dev run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml --log
```

`air` auto-loads a repository-root `.env`. Put real provider settings there:

```dotenv
AIR_MODEL_LOCAL_API_KEY=...
AIR_MODEL_LOCAL_BASE_URL=http://localhost:11434/v1
AIR_MODEL_LOCAL_MODEL=qwen2.5-coder
```

AIR's default model config is `examples/local-openai-compatible.json`, which
uses `AIR_MODEL_LOCAL_*` variables directly. Use `--model-config` when a command
should target a different provider.

The profile points to:

- `examples/simple-helpdesk/helpdesk-rag.air-plan.yaml`
- `examples/simple-helpdesk/module-store.air-store.yaml`
- `examples/simple-helpdesk/input.json`
- `examples/simple-helpdesk/tools.json`
- `examples/local-openai-compatible.json`

Profiles are the preferred user-facing entrypoint because they hide repeated flags.

## 2. Run A Skill

Skills are packaged AIR workflows with instructions, tools, capabilities, and
verification metadata. The built-in general coding skill is `code-agent`:

```bash
cargo run -p air-cli -- skill list
cargo run -p air-cli -- skill validate code-agent
cargo run -p air-cli -- skill explain code-agent
```

To run it:

```bash
cargo run -p air-cli -- run "refactor a helper and run tests"
```

Imported skills are copied and audited before use. AIR does not execute install
scripts during import:

```bash
cargo run -p air-cli -- skill import ./some-skill --out skills/vendor/some-skill
cargo run -p air-cli -- skill audit skills/vendor/some-skill
```

## 3. Module Anatomy

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
- `policy` can add budgets such as `max_tool_calls`, `max_model_calls`, `max_repeated_tool_calls`, `timeout_seconds`, and `require_approval`.

Use `tool_call` when the AIR module knows the exact tool statically. Use `tool_batch_dispatch`
when a model or prior tool emits a bounded array of typed choices shaped like
`{ "tool": "declared.tool", "input": {...} }`. The runtime still requires every selected tool
to be declared in `tools`, checks the
declared capability against `requires.capabilities` and the provider capability, enforces approval
paths and `policy.max_tool_calls` / `policy.max_repeated_tool_calls`, and records the selected tools
in trace metadata. This is the bounded AIR version of an opencode-style plan-act-observe step: the
model can choose the next action, but only inside the module's declared tool boundary.

Object schemas allow undeclared fields by default for compatibility with provider metadata and evolving module contracts. Add `additional_properties: false` to a detailed object schema when the AIR VM should reject undeclared fields.

Conditions use AIR's small equality-only condition DSL. `&&` binds tighter than `||`; parentheses and numeric comparisons are not supported. See [condition_dsl.md](condition_dsl.md) for the formal grammar and limits.

## 4. RunPlan Anatomy

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
object construction, arrays, counts, and coalescing. `coalesce` returns the first non-empty value;
when every available candidate is empty, it returns the last available empty value, which lets plans
express defaults such as `literal: []`. For semantic or lossy interface conversion, make the
conversion an explicit adapter module with a `model_call`, then connect source output to the adapter
input and adapter output to the target module. This keeps model cost, timeout, retry, schema
validation, and trace events visible instead of hiding model execution inside wiring.

## 5. Module Store And Recipes

A module store publishes modules and optional recipes:

```yaml
store:
  name: simple-helpdesk-store
  version: 0.1.0

imports:
  - ../../modules/std/module-store.air-store.yaml

modules:
  helpdesk.rag@0.1.0:
    path: examples/simple-helpdesk/helpdesk-rag.air.yaml
    kind: composite
    visibility: public
    description: "Retrieve local helpdesk documents, then answer with citations."
    covers: [retrieve_docs, answer_question]
    priority: 100
```

`imports` let an application store reuse standard or shared module stores without copying module
refs. Imported modules and recipes are merged before local entries, and duplicate module or recipe
ids are rejected so an import cannot silently shadow another component. Relative import paths are
resolved from the importing store file; relative module paths inside the imported store are resolved
from that imported store file.

Planner behavior:

- public composites are shown before internal primitives;
- `priority` and `covers` help the planner choose large verified components;
- recipes let the planner select a pre-validated topology by `recipe_id`;
- selected recipes are materialized locally and validated before execution.
- `air plan --explain --task "..." --store ...` prints the local component-selection ranking
  without calling a model. Use it to confirm the planner will see the right large component before
  spending a model call or running a bench.

## 6. Dynamic Fan-Out

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

## 7. Model Config

AIR uses OpenAI-compatible model config for real model calls:

```json
{
  "models": {
    "rag_answerer": {
      "base_url": "https://example.com/v1",
      "model": "example-model",
      "temperature": 0,
      "request_timeout_seconds": 1800,
      "json_mode": true,
      "native_tool_calls": false,
      "extra_body": {
        "thinking": {
          "type": "enabled",
          "clear_thinking": false
        }
      },
      "system_prompt": "Return only JSON."
    }
  }
}
```

`request_timeout_seconds` is optional and defaults to 1800 seconds. AIR action `timeout_seconds` is forwarded to timeout-aware providers, and the OpenAI-compatible provider uses the smaller of the provider request timeout and the AIR action timeout as the request deadline. AIR also records elapsed-time violations in the runtime trace.

OpenAI-compatible providers are not identical. AIR does not hard-code behavior for each model name. The provider uses `async-openai` as the single transport path, with BYOT JSON request bodies so AIR can still pass provider extensions. Use `json_mode` or `response_format` for structured-output support, and use `extra_body` to pass provider-specific request fields such as thinking controls, self-hosted gateway flags, or other vendor extensions. AIR merges `extra_body` into the chat/completions request body without overriding the configured AIR fields. When a provider returns text or a wrapper that does not match the declared AIR output schema, the runtime rejects it with schema feedback so retry attempts can correct the response against the same declared interface.

Set `native_tool_calls` to `true` only for providers that support OpenAI chat-completions function tools. In that mode, AIR converts model input fields named `allowed_tools` and `tool_schemas` into OpenAI `tools`, then normalizes returned `tool_calls` back into AIR's `{complete:false, tool_calls:[...]}` decision shape. This is an opt-in single path, not a fallback; if the provider rejects native tools, the model call fails visibly.

For deterministic offline smoke tests, the native CLI also accepts fixture model configs:

```json
{
  "fixtures": {
    "planner": {
      "steps": [
        "inspect inputs",
        "return deterministic fixture output"
      ]
    }
  }
}
```

Fixture models return the configured JSON for a model alias and still go through AIR schema
validation, trace, timeout, and retry handling. They are intended for orchestration tests, not for
quality evaluation.

Run with a matching `.env`:

```dotenv
AIR_MODEL_PROFILE=local
AIR_MODEL_LOCAL_API_KEY=...
AIR_MODEL_LOCAL_BASE_URL=https://example.com/v1
AIR_MODEL_LOCAL_MODEL=example-model
```

```bash
cargo run -p air-cli -- dev run-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
```

Every AIR profile can point at the same shared model config, so examples and local apps do not need to repeat provider settings.

## 8. Tool Config And Capabilities

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
  "workspace_dir": ".",
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
- when tool outputs register artifacts, later `sources`, `citations`, and `source_ids` in model or return outputs refer only to known artifact ids.
- for `tool_batch_dispatch`, every selected tool is one of the module's declared tools and receives
  only its emitted `input` object.

Common native tools live in the `air-tools` crate and are configured through `--tool-config`.
Tool kinds are lower-level capabilities; the names exposed to a model are application aliases.
For the code-agent edit loop, prefer the OpenCode-style aliases `question`, `bash`,
`read`, `glob`, `grep`, `edit`, `write`, `task`, `webfetch`, `todowrite`,
`todoread`, and `skill`. Git operations should use `bash`, the same terminal path
a human developer would use.

| Tool kind | Typical AIR tool name | Input | Output | Notes |
| --- | --- | --- | --- | --- |
| `local_docs_search` | `docs.search` | `{ "query": "..." }` | `{ query, documents[], artifacts[] }` | In-memory docs for examples, tests, and local RAG. |
| `local_reflection` | `research.think` | `{ "reflection": "..." }` | `{ reflection }` | Deterministic reflection placeholder for bounded research loops. |
| `http_json` | `web.search` / custom API | tool input object | JSON response body | Calls REST endpoints; response must be JSON. |
| `web_fetch` | `web.fetch` | `{ "url": "https://..." }` | `{ url, status, content_type, text, bytes, truncated, artifacts[] }` | GET only; supports headers, bearer token env, timeout, max bytes. |
| `playwright_search` | `web.search` | `{ "query": "...", "query_variants": [...], "search_base_url": "https://www.bing.com/search", "cache_dir": "target/search-cache" }` | `{ query, queries, documents[], artifacts[], diagnostics }` | Browser-backed search for research and coding agents; supports query variants, domain filters, per-domain dedupe, concurrent page fetch, timeouts, fixtureable search base URLs, optional TTL page-content cache, selector/latency diagnostics, and artifact output. |
| `playwright_page_audit` | `browser.audit` | `{ "path": "examples/app/index.html", "required_text": ["Product"], "require_canvas": true }` or `{ "url": "https://localhost:3000" }` | `{ success, viewport_count, screenshot_paths[], missing_required_text[], present_forbidden_text[], has_required_canvas, viewports[], diagnostics[], artifacts[] }` | Browser-backed render audit for frontend coding agents. Opens a local file constrained by `base_dir` or an http(s) URL, renders configured desktop/mobile viewports, captures screenshots, checks required/forbidden text, nonblank canvas rendering, console/page errors, horizontal overflow, and coarse overlapping text/click targets. |
| `file_read` | `file.read` | `{ "path": "relative/file.txt", "start_line": 10, "end_line": 80, "line_numbers": true }` or `{ "path": "...", "contains": "symbol", "occurrence": 2, "context_lines": 8 }` | `{ path, content, numbered_content, bytes, start_line, end_line, match_line, contains, occurrence, total_lines, truncated, artifacts[] }` | Read-only and constrained to configured `base_dir`; line ranges, numbered output, and fixed-string `contains` locators are optional. `contains` returns the requested matching line plus bounded surrounding context and fails if the string or occurrence is absent, preventing accidental unrelated reads. Binary and non-UTF-8 files are rejected instead of lossy-decoded into agent context. |
| `file_read_many` | `file.read_many` | `{ "files": ["src/a.rs", { "path": "src/b.rs", "contains": "fn run", "context_lines": 8 }] }` | `{ files[], file_count, bytes, truncated, artifacts[] }` | Batch variant of `file.read` for bounded multi-file context gathering. Each entry is validated with the same path, UTF-8, binary, range, and `contains` rules as `file.read`; `max_files` keeps one call from flooding context. |
| `file_write` | `file.write` | `{ "path": "relative/file.txt", "content": "..." }` | `{ path, bytes, created, overwritten, artifacts[] }` | Write tool constrained to configured `base_dir`; optional directory creation and overwrite policy are set in tool config. |
| `file_edit` | `file.edit` | `{ "filePath": "relative/file.txt", "oldString": "...", "newString": "...", "replaceAll": false }` | `{ path, bytes, replacements, edit_count, match_strategy, match_strategies[], diff, diff_truncated, artifacts[] }` | Edit constrained to configured `base_dir`. The model-facing contract follows the OpenCode-style `filePath` / `oldString` / `newString` shape. AIR keeps fuzzy matching internal: exact matching is tried first, then conservative whitespace-tolerant strategies only when they identify a safe match. Emits bounded unified-diff output for audit. No-op edits and ambiguous multiple matches are rejected unless `replaceAll` is explicitly requested. |
| `repo_files` | `repo.files` | `{ "query": "...", "path": "optional/dir" }` | `{ repo, query, files[], truncated, artifacts[] }` | Read-only repository file listing through `rg --files`; optional query/path/glob filtering. |
| `repo_search` | `repo.search` | `{ "query": "...", "mode": "fixed", "path": "optional/dir", "max_matches": 20 }` | `{ repo, query, mode, matches[], truncated, artifacts[] }` | Read-only search through `rg`; defaults to fixed-string mode and supports explicit `mode: "regex"` for grep-style code discovery. Per-call `max_matches` is capped by tool config. Returns path/line/column/text matches. |
| `repo_symbols` | `repo.symbols` | `{ "query": "...", "path": "optional/dir", "glob": "*.rs" }` | `{ repo, query, symbols[], truncated, artifacts[] }` | Lightweight repository symbol map through `rg`, covering common declarations such as functions, classes, structs, enums, interfaces, types, constants, and variables. This is intentionally simpler than LSP and works without language servers. |
| `repo_references` | `repo.references` | `{ "symbol": "Identifier", "path": "optional/dir", "glob": "*.rs", "context_lines": 4 }` | `{ repo, symbol, definitions[], references[], snippets[], truncated, artifacts[] }` | LSP-lite identifier lookup through `rg`; filters token boundaries, marks declaration-like matches, and returns nearby snippets without requiring a language server. |
| `rust_analyzer` | `lsp` | `{ "command": "references", "path": "src/lib.rs", "symbol": "helper" }` or `{ "command": "diagnostics", "path": "src/lib.rs" }` | References or diagnostics output from rust-analyzer. | Optional Rust LSP wrapper using one familiar `lsp` alias. It reuses a cached rust-analyzer session for references and supports diagnostics for correction loops. |
| `repo_context` | `repo.context` | `{ "query": "...", "mode": "fixed", "context_lines": 8 }` | `{ repo, query, mode, matches[], snippets[], truncated, artifacts[] }` | Read-only code context through `rg`; defaults to fixed-string mode and supports explicit `mode: "regex"`. Groups matches by file and returns nearby numbered snippets with a `code_context` artifact. |
| `context_measure` | `context.measure` | `{ "payload": {...}, "max_context_chars": 200000, "threshold_percent": 80 }` | `{ chars, max_context_chars, threshold_percent, threshold_chars, usage_ratio, should_compact, fields[], artifacts[] }` | Deterministically estimates serialized context size and returns whether a module should route through a semantic compaction step. This is generic and can be used by coding, research, planning, or support agents. |
| `artifact_validate` | `artifact.validate` | `{ "evidence": {...}, "registered_ids": ["doc-1"], "citations": {...} }` | `{ valid, registered_ids[], cited_ids[], missing_ids[], unused_registered_ids[], registered_artifacts[], artifacts[] }` | Validates that model-produced `source_ids`, `citations`, or `artifact_ids` refer only to registered artifact ids. Set `fail_on_missing: true` in tool config for fail-closed provenance checks after semantic adapters or compaction. |
| `command_run` | `test` | `{}` for a single configured command; advanced configs may accept constrained parameters | `{ command, argv, success, status, log, diagnostics[], bytes, truncated, full_log_path, truncation_hint, artifacts[] }` | Runs only allowlisted argv arrays from tool config; no shell interpolation. Code-agent profiles should expose `test` as one semantic validation tool rather than asking the model to select validation variants. Configured argv parts may use constrained parameters for bounded test names or paths in specialized tools. Extracts common Rust/TypeScript/file-line diagnostics for edit loops. When output is truncated, the complete command output is saved under `.air/tool-output/` and `full_log_path` points to it so agents can inspect targeted sections with the configured search/read aliases such as `grep` and `read`. Set `truncation_direction: "tail"` in tool config when the end of a test/build log is usually most useful. |

The reusable `modules/std/context/compact.air.yaml` module wraps `context.measure` with conditional
routing. Under budget it returns the raw payload; over budget it calls the shared `context_compactor`
model and returns a bounded brief with retained facts, source ids, open tasks, risks, and
limitations. Coding agents, research agents, support agents, and planner agents can all compose this
standard module instead of each defining their own compaction state machine.

Artifact-producing tools return a common shape:

```json
{
  "artifacts": [
    {
      "id": "doc-1",
      "kind": "web_page | browser_screenshot | doc_chunk | file_span | file_write | file_edit | repo_listing | repo_search | repo_symbols | repo_references | code_context | diagnostic_context | todo_list | context_measure | test_log",
      "title": "Readable title",
      "uri": "file-or-web-location",
      "content": "Evidence text",
      "metadata": {
        "provider": "local_docs"
      }
    }
  ]
}
```

The native AIR VM records artifact ids from `artifacts[]` and from compatible `documents[]`
results. If the registry is non-empty, model outputs and return outputs that contain
`sources`, `citations`, or `source_ids` arrays must cite those ids. This gives deep research a
checked source list and gives coding agents checked references to file reads, diffs, fetches, or
test logs. `artifact.validate` provides the same check as an explicit AIR tool when a module has
passed through semantic compaction or adapter layers and wants to validate against carried-forward
`source_ids`. Existing modules without artifact-producing tools continue to run without citation
enforcement.

The `skills/code-agent` workflow exposes one OpenCode-style loop over declared tools such as
`question`, `bash`, `read`, `glob`, `grep`, `edit`, `write`, `task`, `webfetch`,
`todowrite`, `todoread`, and `skill`.
The same loop handles exploration, review, editing,
formatting, and verification from a single task prompt; the model discovers relevant files through
tools instead of receiving target-file hints from the CLI. The model-facing write contract stays
small: `edit(filePath, oldString, newString, replaceAll?)`; AIR keeps matching strategy,
read-before-write checks, formatting, verification, and trace capture in
the runtime/tool layer while the workflow remains one generic edit loop.

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

For browser-backed research, use `playwright_search`. This is useful when a coding or research
agent needs current documentation, GitHub pages, release notes, issues, or general web evidence
without depending on a paid search API. `query_variants` can use the same `{{field}}` template
syntax as HTTP tools. Install the browser runtime once with `npm install` and
`npx playwright install chromium`.

```json
{
  "tools": {
    "web.search": {
      "kind": "playwright_search",
      "capability": "network.search",
      "script_path": "../../tools/playwright/search.cjs",
      "query_variants": [
        "{{query}} GitHub",
        "{{query}} documentation",
        "{{query}} issues releases"
      ],
      "max_results": 8,
      "max_results_per_query": 6,
      "max_per_domain": 2,
      "max_content_chars": 12000,
      "exclude_domains": ["facebook.com", "x.com", "twitter.com"],
      "required_terms": [],
      "exclude_terms": [],
      "page_concurrency": 3,
      "navigation_timeout_ms": 60000,
      "overall_timeout_ms": 600000,
      "search_delay_ms": 500,
      "retry_count": 1,
      "fetch_pages": true,
      "timeout_seconds": 600
    }
  }
}
```

The tool also accepts per-call overrides in its input, including `query_variants`,
`include_domains`, `exclude_domains`, `max_results`, `max_results_per_query`,
`max_per_domain`, `required_terms`, `exclude_terms`, `max_content_chars`,
`page_concurrency`, `navigation_timeout_ms`, `overall_timeout_ms`, `search_delay_ms`, `retry_count`, `fetch_pages`, and
`user_agent`. The output `diagnostics` field records search runs, fetch failures,
domain filters, and result counts so the trace shows whether the agent actually found enough
evidence.

Example coding-agent file, search, edit, and bash tool aliases:

```json
{
  "tools": {
    "read": {
      "kind": "file_read",
      "capability": "file.read",
      "base_dir": ".",
      "max_bytes": 32768
    },
    "grep": {
      "kind": "file_search",
      "capability": "file.read",
      "base_dir": ".",
      "max_matches": 40,
      "max_context_lines": 2,
      "max_bytes": 32768
    },
    "edit": {
      "kind": "file_edit",
      "capability": "file.write",
      "base_dir": ".",
      "max_bytes": 262144
    },
    "glob": {
      "kind": "repo_files",
      "capability": "code.read",
      "repo_dir": ".",
      "max_files": 100
    },
    "bash": {
      "kind": "bash",
      "capability": "code.test",
      "cwd": ".",
      "timeout_seconds": 600,
      "max_bytes": 65536,
      "truncation_direction": "tail"
    }
  }
}
```

When `workspace_dir` is set, relative `base_dir`, `repo_dir`, `root_dir`, and command `cwd`
values are resolved from that workspace. `workspace_dir: "."` resolves to the current git
workspace root when one is available, so the same config works from subdirectories. Without
it, relative tool paths remain relative to the tool config file for small colocated examples.

## 9. Approvals

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

## 10. Traces, Checkpoints, Resume

Use logs for humans:

```bash
cargo run -p air-cli -- dev run-plan --profile examples/deep-research/profile.air-profile.yaml --log
```

Use JSONL traces for replay/audit. Trace files are redacted by default: common sensitive keys such as API keys, authorization headers, passwords, secrets, and tokens are masked, and large string/event payloads are capped.

```bash
cargo run -p air-cli -- dev run-plan --profile examples/deep-research/profile.air-profile.yaml \
  --trace-out target/generated/run.trace.jsonl
```

Code-agent artifacts can be audited directly:

```bash
cargo run -p air-cli -- run "refactor a helper and run tests" \
  --artifact-out target/generated/code-runs/helper

cargo run -p air-cli -- audit run target/generated/code-runs/helper \
  --report target/generated/code-runs/helper-audit.md
```

`air audit run` emits `air.audit.v1` JSON and optionally writes a Markdown
summary. The audit is deterministic and read-only. It checks the code-run
artifact, referenced trace, workspace diff, `CodeRunVerdict`, verification
events, tool call counts, context/cost metrics, and reward-hacking indicators
such as protected eval or trust-state file changes. Missing traces,
verification failures, forbidden-file changes, and protected eval mutations are
findings in the report; LLM-generated explanations should be treated as advisory
only.

For a group of runs, collect audits into a platform review report:

```bash
cargo run -p air-cli -- audit collect \
  --from target/generated \
  --since-unix 1770000000 \
  --limit 100 \
  --out-dir .air/audit/latest \
  --report .air/audit/latest.md
```

The collection writes `.air/audit/latest/collection.json`, per-run audit JSON
under `.air/audit/latest/runs/`, and a Markdown report. It summarizes pass/fail
counts, finding categories, severity counts, skill usage, tool usage, top cost
runs, high-risk runs, scan-window metadata, and collection errors. Use
`--since-unix` and `--limit` for periodic reviews so large artifact trees do not
need to be fully re-scanned every time. This is the periodic self-improve intake:
platform teams review the report, promote real failures into regressions,
evaluate candidate fixes, and only promote changes through PRs.

For the same review window, Dream runs audit collection and improve mining
together and writes a single platform report:

```bash
cargo run -p air-cli -- dream run \
  --from target/generated \
  --limit 100 \
  --out-dir .air/dream/latest \
  --write-regressions
```

Dream's output schema is `air.dream.v1`; the saved cursor schema is
`air.dream_state.v1` and can be inspected with:

```bash
cargo run -p air-cli -- dream state
```

Treat Dream as the night-cycle consolidation layer: current Dream reports turn
traces into findings and regression candidates; future memory systems can add
another consolidation stage for durable team/project memories without changing
the promotion rule. Source changes still go through explicit candidate patches,
evaluation, compare, and manual PR review.

## 11. Eval Integrity

Use an eval manifest when AIR improvements should be compared against a stable
corpus:

```bash
cargo run -p air-cli -- eval manifest \
  --out .air/evals/manifest.json

cargo run -p air-cli -- eval check \
  --manifest .air/evals/manifest.json
```

By default the manifest pins AIR's code-agent benchmark suites, promoted
regressions, and git-tracked evaluation/scoring code discovered from the current
repository. The protected path list combines broad eval/trust globs with the
actual pinned files, so a source-file move is picked up when the manifest is
regenerated instead of depending on one hard-coded path. You can add company
suites with repeated `--include` flags:

```bash
cargo run -p air-cli -- eval manifest \
  --out .air/evals/manifest.json \
  --include skills/code-agent/benches \
  --include .air/company-evals
```

The check validates pinned file hashes and also inspects the current git diff
for protected eval paths. `air improve evaluate` runs this check automatically
when `.air/evals/manifest.json` exists, so candidate patches cannot quietly
change the eval corpus or trust state to pass.

Prepare candidate slots before trying multiple fixes:

```bash
cargo run -p air-cli -- self prepare IMP-001 --candidates 3
cargo run -p air-cli -- self fix IMP-001 --candidates 3 --evaluate
cargo run -p air-cli -- self fix IMP-001 --skill code-agent --candidates 3 --evaluate
cargo run -p air-cli -- self capture IMP-001 cand-1
```

`self fix` runs AIR's own `code-agent` in detached worktrees and writes each
candidate patch back into `.air/candidates/IMP-001/cand-N/patch.diff`. The
candidate worktree first receives the caller's current tracked and untracked
workspace overlay, then AIR captures only the patch generated after that
bootstrap commit. The candidate task is not a generic "fix this" prompt: AIR
injects the selected finding category, impact score, priority reason, regression
fixture hint, benchmark task context, evidence, and previous rejected-candidate
feedback into `.air/candidates/IMP-001/cand-N/task.md`. Benchmark suites,
fixtures, regressions, eval manifests, and `skills.lock` are evidence/gates, not
acceptable candidate fix targets. Use `--skill <skill-id>` for local skill
improvements such as prompt, profile, tool policy, docs, tests, or regressions.
AIR does not modify external MCP servers; it can improve the local AIR layer
that routes, constrains, audits, and benchmarks those MCP tools. Use `self
capture` when a human or another agent produced the candidate in the current
workspace. After each candidate is evaluated with `air improve evaluate`,
save its JSON output as
`.air/candidates/IMP-001/cand-N/eval.json`, then compare them:

```bash
cargo run -p air-cli -- self compare IMP-001 \
  --out .air/candidates/IMP-001.md
```

The comparison ranks accepted candidates above `needs_review`, then rejected
candidates, and writes a scorecard with patch path, changed-file count,
regression, test, guard, blocked, and warning counts. Promotion remains a PR
step rather than an automatic write to main.

Use raw traces only for trusted local debugging. Raw traces preserve complete model/tool inputs and outputs, which can include prompts, documents, credentials, or user data.

```bash
cargo run -p air-cli -- dev run-plan --profile examples/deep-research/profile.air-profile.yaml \
  --trace-out target/generated/run.raw.trace.jsonl \
  --trace-raw
```

Use checkpoint/resume for long plans:

```bash
cargo run -p air-cli -- dev run-plan --profile examples/deep-research/profile.air-profile.yaml \
  --checkpoint-out target/generated/run.state.json

cargo run -p air-cli -- dev resume-plan --profile examples/deep-research/profile.air-profile.yaml \
  --state target/generated/run.state.json \
  --override clarify.clarification.normalized_question='"Updated question"'
```

Use JIT hot-path cache for stable dynamic topologies:

```bash
cargo run -p air-cli -- dev run-plan --profile examples/deep-research/profile.air-profile.yaml \
  --jit-cache target/generated/jit_cache
```

AIR specializes the resolved topology, validates it, and reuses the static hot path for matching inputs. It does not cache model outputs.

## 12. Verification

Run the workspace checks:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Use targeted example smoke checks when needed:

```bash
bash examples/deep-research/dev/verify.sh
```
