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
- `policy` can add budgets such as `max_tool_calls`, `max_model_calls`, `max_repeated_tool_calls`, `timeout_seconds`, and `require_approval`.

Use `tool_call` when the AIR module knows the exact tool statically. Use `tool_dispatch` when a
model or prior tool emits a typed choice shaped like `{ "tool": "declared.tool", "input": {...} }`.
Use `tool_batch_dispatch` when it emits a bounded array of those choices and the calls are
independent. The runtime still requires every selected tool to be declared in `tools`, checks the
declared capability against `requires.capabilities` and the provider capability, enforces approval
paths and `policy.max_tool_calls` / `policy.max_repeated_tool_calls`, and records the selected tools
in trace metadata. This is the bounded AIR version of an opencode-style plan-act-observe step: the
model can choose the next action, but only inside the module's declared tool boundary.

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
object construction, arrays, counts, and coalescing. `coalesce` returns the first non-empty value;
when every available candidate is empty, it returns the last available empty value, which lets plans
express defaults such as `literal: []`. For semantic or lossy interface conversion, make the
conversion an explicit adapter module with a `model_call`, then connect source output to the adapter
input and adapter output to the target module. This keeps model cost, timeout, retry, schema
validation, and trace events visible instead of hiding model execution inside wiring.

## 4. Module Store And Recipes

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

`request_timeout_seconds` is optional and defaults to 120 seconds. AIR action `timeout_seconds` is forwarded to timeout-aware providers, and the OpenAI-compatible provider uses the smaller of the provider request timeout and the AIR action timeout as the request deadline. AIR also records elapsed-time violations in the runtime trace.

OpenAI-compatible providers are not identical. AIR does not hard-code behavior for each model name. The provider uses `async-openai` as the single transport path, with BYOT JSON request bodies so AIR can still pass provider extensions. Use `json_mode` or `response_format` for structured-output support, and use `extra_body` to pass provider-specific request fields such as thinking controls, self-hosted gateway flags, or other vendor extensions. AIR merges `extra_body` into the chat/completions request body without overriding the configured AIR fields. When a provider returns text or a wrapper that does not match the declared AIR output schema, the runtime rejects it with schema feedback so retry attempts can repair the response against the same declared interface.

Set `native_tool_calls` to `true` only for providers that support OpenAI chat-completions function tools. In that mode, AIR converts model input fields named `allowed_tools` and `tool_schemas` into OpenAI `tools`, then normalizes returned `tool_calls` back into AIR's `{complete:false, tool_calls:[...]}` decision shape. This is an opt-in single path, not a fallback; if the provider rejects native tools, the model call fails visibly.

For deterministic offline smoke tests, the native CLI also accepts fixture model configs:

```json
{
  "fixtures": {
    "code_reviewer": {
      "summary": "fixture review",
      "findings": [],
      "source_ids": [],
      "search_quality": { "sufficient": true, "gaps": [] },
      "next_steps": []
    }
  }
}
```

Fixture models return the configured JSON for a model alias and still go through AIR schema
validation, trace, timeout, and retry handling. They are intended for orchestration tests, not for
quality evaluation.

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
- when tool outputs register artifacts, later `sources`, `citations`, and `source_ids` in model or return outputs refer only to known artifact ids.
- for `tool_dispatch` and `tool_batch_dispatch`, every selected tool is one of the module's declared
  tools and receives only its emitted `input` object.

Common native tools live in the `air-tools` crate and are configured through `--tool-config`.

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
| `file_write` | `file.write` | `{ "path": "relative/file.txt", "content": "..." }` | `{ path, bytes, created, overwritten, artifacts[] }` | Write tool constrained to configured `base_dir`; optional directory creation, overwrite policy, and read-before-overwrite policy are set in tool config. When `require_read` is enabled, overwrites are rejected if the file changed after the last `file.read`. |
| `file_edit` | `file.edit` | `{ "path": "relative/file.txt", "old_string": "...", "new_string": "...", "match_strategy": "exact" }` or `{ "path": "...", "edits": [{ "old_string": "...", "new_string": "..." }] }` | `{ path, bytes, replacements, edit_count, match_strategy, match_strategies[], diff, diff_truncated, artifacts[] }` | Edit constrained to configured `base_dir`; requires a fresh prior `file.read` by default. The default `exact` strategy uses literal matching; explicit `line_trimmed` or `indentation_flexible` strategies support conservative whitespace-tolerant block replacement. `edits[]` applies multiple same-file replacements atomically: any failed edit rejects the call without writing. Emits bounded unified-diff output for audit. No-op edits and ambiguous multiple matches are rejected unless `replace_all` is explicitly allowed. |
| `file_ops` | `file.ops` | `{ "operations": [{ "kind": "edit", "path": "...", "old_string": "...", "new_string": "..." }], "dry_run": true }` | `{ repo, success, checked, applied, files[], file_count, diagnostics[], match_strategies[], diff, diff_truncated, artifacts[] }` | Atomic multi-file structured edits/writes constrained to configured `base_dir`; requires fresh prior reads for existing files when enabled. Edit operations default to conservative `auto` matching: exact first, then whitespace-tolerant strategies only when they identify a safe match. Failed dry-runs return `success:false` diagnostics without mutating files. |
| `file_patch` | `file.patch` | `{ "patch": "diff --git ...", "dry_run": true }` | `{ repo, success, checked, applied, files[], file_count, diagnostics[], bytes, artifacts[] }` | Applies a unified diff through `git apply --check` then `git apply`; validates changed paths, max files, new/delete policy, and fresh read-before-patch for existing files. In `dry_run` mode failed patch checks return `success:false` with diagnostics instead of mutating files. |
| `git_diff` | `git.diff` | `{ "path": "...", "paths": ["..."], "files": [{ "path": "..." }], "staged": false }` | `{ repo, diff, bytes, truncated, artifacts[] }` | Read-only diff; path filters must stay inside `repo_dir`. The `files` form accepts `file.patch` changed-file objects so coding agents can audit only the paths they changed instead of the whole dirty workspace. |
| `git_status` | `git.status` | `{}` | `{ repo, clean, entries[], file_count, truncated, artifacts[] }` | Read-only workspace status through `git status --porcelain=v1`; returns structured index/worktree entries for review, edit, and final summaries. |
| `repo_files` | `repo.files` | `{ "query": "...", "path": "optional/dir" }` | `{ repo, query, files[], truncated, artifacts[] }` | Read-only repository file listing through `rg --files`; optional query/path/glob filtering. |
| `repo_search` | `repo.search` | `{ "query": "...", "mode": "fixed", "path": "optional/dir", "max_matches": 20 }` | `{ repo, query, mode, matches[], truncated, artifacts[] }` | Read-only search through `rg`; defaults to fixed-string mode and supports explicit `mode: "regex"` for grep-style code discovery. Per-call `max_matches` is capped by tool config. Returns path/line/column/text matches. |
| `repo_symbols` | `repo.symbols` | `{ "query": "...", "path": "optional/dir", "glob": "*.rs" }` | `{ repo, query, symbols[], truncated, artifacts[] }` | Lightweight repository symbol map through `rg`, covering common declarations such as functions, classes, structs, enums, interfaces, types, constants, and variables. This is intentionally simpler than LSP and works without language servers. |
| `repo_references` | `repo.references` | `{ "symbol": "Identifier", "path": "optional/dir", "glob": "*.rs", "context_lines": 4 }` | `{ repo, symbol, definitions[], references[], snippets[], truncated, artifacts[] }` | LSP-lite identifier lookup through `rg`; filters token boundaries, marks declaration-like matches, and returns nearby snippets without requiring a language server. |
| `repo_context` | `repo.context` | `{ "query": "...", "mode": "fixed", "context_lines": 8 }` | `{ repo, query, mode, matches[], snippets[], truncated, artifacts[] }` | Read-only code context through `rg`; defaults to fixed-string mode and supports explicit `mode: "regex"`. Groups matches by file and returns nearby numbered snippets with a `code_context` artifact. |
| `diagnostic_context` | `diagnostic.context` | `{ "diagnostics": [{ "path": "src/lib.rs", "line": 42 }], "context_lines": 4 }` | `{ repo, diagnostics[], snippets[], unreadable[], truncated, artifacts[] }` | Converts structured command diagnostics into nearby source snippets. Paths must resolve inside `repo_dir`; unreadable, missing, or out-of-bounds diagnostics are reported in `unreadable[]` instead of leaking outside the repository. |
| `todo_write` | `todo.write` | `{ "todos": [{ "id": "inspect", "content": "...", "status": "in_progress", "priority": "high" }] }` | `{ todos[], total, open_count, pending_count, in_progress_count, completed_count, cancelled_count, artifacts[] }` | Writes a structured task-progress artifact for complex agents. Status must be `pending`, `in_progress`, `completed`, or `cancelled`; priority must be `high`, `medium`, or `low`; at most one item may be `in_progress`. |
| `todo_read` | `todo.read` | `{}` | `{ todos[], total, open_count, pending_count, in_progress_count, completed_count, cancelled_count, artifacts[] }` | Reads the current in-memory todo list from the configured tool provider. This mirrors opencode-style task tracking without adding task state to the AIR IR. |
| `context_measure` | `context.measure` | `{ "payload": {...}, "max_context_chars": 200000, "threshold_percent": 80 }` | `{ chars, max_context_chars, threshold_percent, threshold_chars, usage_ratio, should_compact, fields[], artifacts[] }` | Deterministically estimates serialized context size and returns whether a module should route through a semantic compaction step. This is generic and can be used by coding, research, planning, or support agents. |
| `artifact_validate` | `artifact.validate` | `{ "evidence": {...}, "registered_ids": ["doc-1"], "citations": {...} }` | `{ valid, registered_ids[], cited_ids[], missing_ids[], unused_registered_ids[], registered_artifacts[], artifacts[] }` | Validates that model-produced `source_ids`, `citations`, or `artifact_ids` refer only to registered artifact ids. Set `fail_on_missing: true` in tool config for fail-closed provenance checks after semantic adapters or compaction. |
| `command_run` | `test.run` | `{ "command": "alias" }` or `{ "command": "alias_with_filter", "test_filter": "module::case" }` | `{ command, argv, success, status, log, diagnostics[], bytes, truncated, full_log_path, truncation_hint, artifacts[] }` | Runs only allowlisted argv arrays from tool config; no shell interpolation. Configured argv parts may use constrained `{{parameter}}` placeholders for bounded test names or paths. Extracts common Rust/TypeScript/file-line diagnostics for edit loops. When output is truncated, the complete command output is saved under `.air/tool-output/` and `full_log_path` points to it so agents can inspect targeted sections with `file.search` or bounded `file.read`. Set `truncation_direction: "tail"` in tool config when the end of a test/build log is usually most useful. |

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
      "kind": "web_page | browser_screenshot | doc_chunk | file_span | file_write | file_edit | file_ops | file_patch | repo_listing | repo_search | repo_symbols | repo_references | code_context | diagnostic_context | todo_list | context_measure | git_diff | git_status | test_log",
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

The `examples/code-agent` workflows show four coding-agent patterns: plan agents turn open goals
into bounded task graphs and acceptance checks; explore agents answer repository questions without
write capability; review agents search external references, inspect repository context, run one
allowlisted verification command, and cite exact artifact ids; edit agents use a bounded loop over
declared tools such as `repo.search`, `file.search`, `file.ops`, `test.run`, browser audit tools,
and `git.diff` to make constrained changes and verify them.

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
      "script_path": "../../scripts/playwright_search.cjs",
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
      "navigation_timeout_ms": 12000,
      "overall_timeout_ms": 110000,
      "search_delay_ms": 500,
      "retry_count": 1,
      "fetch_pages": true,
      "timeout_seconds": 120
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
    "file.write": {
      "kind": "file_write",
      "capability": "file.write",
      "base_dir": ".",
      "create_dirs": true,
      "allow_overwrite": false,
      "require_read": true,
      "max_bytes": 262144
    },
    "file.edit": {
      "kind": "file_edit",
      "capability": "file.write",
      "base_dir": ".",
      "allow_replace_all": false,
      "max_bytes": 262144
    },
    "file.ops": {
      "kind": "file_ops",
      "capability": "file.write",
      "base_dir": ".",
      "require_read": true,
      "allow_new_files": true,
      "allow_overwrite": true,
      "allow_replace_all": false,
      "max_files": 20,
      "max_bytes": 262144
    },
    "file.patch": {
      "kind": "file_patch",
      "capability": "file.write",
      "repo_dir": ".",
      "require_read": true,
      "allow_new_files": true,
      "allow_delete_files": false,
      "max_files": 20,
      "max_bytes": 262144
    },
    "git.diff": {
      "kind": "git_diff",
      "capability": "code.read",
      "repo_dir": ".",
      "max_bytes": 262144
    },
    "git.status": {
      "kind": "git_status",
      "capability": "code.read",
      "repo_dir": ".",
      "max_files": 200
    },
    "repo.context": {
      "kind": "repo_context",
      "capability": "code.read",
      "repo_dir": ".",
      "max_matches": 80,
      "max_files": 8,
      "context_lines": 8,
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
