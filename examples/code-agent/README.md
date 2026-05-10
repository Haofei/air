# Code Agent Example

This example contains bounded AIR coding agents and one preferred composed review recipe:

- `code.explore@0.1.0` is a read-only exploration subagent for open-ended repository questions.
  It mirrors the opencode pattern of delegating broad codebase search to a specialized child agent:
  gather web/repo/file/symbol context, then return a concise answer with exact source ids.
- `code.review_with_std_context@0.1.0` is the preferred review recipe. It composes
  `code.review_gather@0.1.0`, the shared `context.compact@0.1.0` standard module, and
  `code.review_analyze@0.1.0`.
- `code.review@0.1.0` is the older monolithic review module kept for comparison.
- `code.core_repair@0.1.0` is the preferred repair recipe and the smallest
  opencode-style core loop: run read-only exploration, pass that output through an explicit
  semantic adapter that selects bounded repair context files, then invoke `code.repair@0.1.0`
  to patch and retest.
- `code.repair@0.1.0` reads a target file plus bounded related files, runs an allowlisted test,
  uses structured diagnostics to gather nearby source context, generates a unified diff, validates
  it with `file.patch` dry-run, applies it through constrained `file.patch`, then retests with one
  bounded retry pass if the first patch does not fix the test. It finishes by calling `git.status`
  so the returned summary includes workspace cleanliness and changed files.
- `code.build_page@0.1.0` generates one static HTML file, writes it through constrained `file.write`,
  runs an allowlisted smoke test, renders desktop/mobile screenshots through `browser.audit`, and
  gets one bounded revision pass if either the smoke test or browser audit fails. Its output includes
  `smoke_log`, `audit_diagnostics`, `screenshots`, and `revised` so callers can judge a failed build
  without digging through the raw trace.

The explore/review agents use:

- `todo.write` for a structured progress artifact before evidence gathering;
- `todo.read` for re-reading the current task-progress artifact before analysis;
- `web.search` for external documentation or issues;
- `repo.files` for relevant repository paths;
- `repo.search` for symbol or text matches, with explicit regex mode available for grep-style discovery;
- `repo.symbols` for a lightweight repository symbol map without requiring LSP setup;
- `repo.references` for LSP-lite definition/reference lookup around an identifier token, with bounded snippets;
- `repo.context` for automatically selected nearby code snippets around repository matches, also with explicit regex mode;
- `file.read` for the target source file, with optional numbered output for diagnostics;
- `file.read_many` for bounded related-file context, such as nearby configs, fixtures, or tests;
- `git.diff` for local changes to that file.
- `git.status` for structured workspace change awareness.
- `test.run` for an allowlisted verification command.

Before final analysis, the preferred composed review plan sends the evidence bundle through the
shared `modules/std/context/compact.air.yaml` module. That module first runs deterministic
`context.measure`; if the bundle crosses the configured threshold, the `context_compactor` model
converts it into a bounded context object with retained facts and exact `source_ids`. If the bundle
is under threshold, the standard module returns the raw payload in `context.raw_payload` and skips
the extra model call. This keeps context growth explicit, reusable, and auditable without adding a
new AIR instruction or baking compaction into one coding agent.

For editing agents, prefer the opencode-style tool split already available in `air-tools`:
`todo.write` and `todo.read` for explicit task tracking on non-trivial work,
`file.read` for context, including `contains` + `occurrence` + `context_lines` when the agent has a symbol or error
string but should not guess line numbers, `file.read_many` for bounded multi-file context gathering,
`file.edit` for exact-string changes that require a prior read by default and explicit
whitespace-tolerant strategies for indentation drift. Use `file.edit` with `edits[]` for atomic
multi-point changes in one file; if any edit fails, the file is left unchanged. Each edit still
emits bounded diff output for audit,
`file.patch` for reviewed multi-file unified diffs, and `file.write` for bounded file creation or
explicit overwrites. Keep shell execution behind `command_run` aliases instead of giving the model
a raw shell. `command_run` returns both raw logs and structured `diagnostics[]`, so repair loops can
focus on file/line/column errors instead of re-parsing terminal output from scratch.
`diagnostic.context` turns those diagnostics into bounded nearby source snippets, which keeps repair
models grounded without forcing them to calculate line ranges by hand.
`command_run` can also expose constrained argv templates such as
`["cargo", "test", "-q", "-p", "air-tools", "{{test_filter}}"]`; every placeholder must have a
declared parameter policy, so agents can target one test without receiving raw shell access.
When `require_read` is enabled, `file.write`, `file.edit`, and `file.patch` reject edits to files
that were not read or were modified after the last read. `file.patch` also supports `dry_run: true`
for `git apply --check` validation without mutating files; the repair agent uses that check before
every patch apply.
For frontend agents, `browser.audit` uses Playwright to open a local file or URL, capture desktop
and mobile screenshots, and return structured layout diagnostics for console errors, page errors,
horizontal overflow, and coarse text/click-target overlap. The build-page agent feeds those
diagnostics back into one bounded revision pass instead of relying only on string checks.

The default `tools.json` uses deterministic local search documents so release verification does
not depend on network access. For real research, switch to `tools.playwright.json`.
`model-fixtures.json` provides deterministic schema-valid outputs for `code_explorer`,
`context_compactor`, and `code_reviewer`, so the explore and composed review plans can run
end-to-end offline in CI without an API key.
The verification script also runs `scripts/playwright_search_fixture_test.cjs` and
`scripts/playwright_page_audit_fixture_test.cjs` when a local
Playwright Chromium browser is installed; that fixture serves Bing-like HTML from localhost and
checks real DOM extraction, URL normalization, page fetch, screenshot capture, layout diagnostics,
and artifact output without external network access. `tools.playwright.json` enables a TTL page-content cache under
`target/generated/playwright_search_cache` so repeated research loops avoid re-fetching the same
result pages.

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/profile.air-profile.yaml

cargo run -p air-cli -- run-plan examples/code-agent/code-review-composed.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/input.json \
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config examples/code-agent/tools.playwright.json \
  --trace-out target/generated/code_agent.trace.jsonl
```

Run the composed review path fully offline with deterministic model fixtures:

```bash
cargo run -p air-cli -- run-plan examples/code-agent/code-review-composed.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/input.json \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json
```

Run the read-only exploration subagent:

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/explore.air-profile.yaml

cargo run -p air-cli -- run-plan --profile examples/code-agent/explore.air-profile.yaml --log
```

Check which high-level coding component the planner will see first without calling a model:

```bash
cargo run -p air-cli -- plan --explain \
  --store examples/code-agent/module-store.air-store.yaml \
  --task "Fix a failing test using diagnostics and retest"
```

This is the code-agent primary routing layer. It mirrors opencode's shape: a primary agent first
chooses a large component or recipe, then only falls back to smaller building blocks when no large
component covers the task. `plan --explain` is deterministic and model-free, so bench harnesses can
assert that review, repair, page-build, and read-only exploration tasks route to the intended AIR
component before any model is asked to generate a RunPlan.

For fixes, the intended first choice is the repair recipe rather than the primitive repair module.
That proves the core coding loop as a reusable AIR graph:

```text
explore -> repair_context semantic adapter -> repair -> test status
```

The adapter is deliberately a normal AIR module. Complex interface conversion, such as turning
`relevant_files[{path, reason}]` into a small `related_files[]` list, stays auditable as a model call
with typed output instead of becoming hidden linker behavior.

Use this as the first coding-agent shape for bench work. It is intentionally static and bounded so
search quality, source grounding, and local-code evidence can be tested.

## User input shape

The user-facing interface should be a natural-language task plus a small typed context object. The
planner uses the task to pick a large component or recipe; the profile supplies model/tool policy.
For a repair task, the current input looks like this:

```json
{
  "task": "fix the failing add function using repository exploration, structured diagnostics, a bounded patch, and retest",
  "query": "repair fixture add function test",
  "target_path": "examples/code-agent/repair-fixture/math.js",
  "related_files": [],
  "test_command": "repair_fixture_test"
}
```

Run the user-facing wrapper. The default recipe is `auto`: `--output` selects build,
`--test` selects repair, review-specific search flags select review, and ambiguous tasks fall
back to read-only exploration.

```bash
cargo run -p air-cli -- code "fix the failing add function and retest" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --related examples/code-agent/repair-fixture/test.js \
  --log
```

The same command can select the other public coding-agent recipes:

```bash
cargo run -p air-cli -- code "explore command_run safety" \
  --target crates/air-tools/src/lib.rs \
  --query command_run

cargo run -p air-cli -- code "review the Playwright search tool" \
  --recipe review \
  --target scripts/playwright_search.cjs \
  --query playwright_search \
  --search-query "Playwright browser search result extraction timeout Node.js" \
  --required-term playwright

cargo run -p air-cli -- code "build a premium product landing page" \
  --recipe build \
  --output examples/apple-landing/index.html \
  --brand Apple \
  --product "Apple Nova"
```

The wrapper only assembles the same typed input and runs the selected AIR recipe. The AIR module
still owns permissions, bounded tool calls, trace, retry, and output schema. Use `--profile` to
select another coding recipe, and `--model-config` / `--tool-config` to override the profile's
providers.

Build the Apple-style landing page:

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/apple-build.air-profile.yaml

cargo run -p air-cli -- run-plan --profile examples/code-agent/apple-build.air-profile.yaml --log
```

Run the bounded repair loop on a tiny fixture:

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/repair.air-profile.yaml

cargo run -p air-cli -- run-plan --profile examples/code-agent/repair.air-profile.yaml --log
```

The repair fixture intentionally starts with a failing implementation so the agent has a concrete
diagnostic to fix. Running the repair profile modifies `examples/code-agent/repair-fixture/math.js`.

Run the preferred core repair loop, including exploration and context selection:

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/repair-core.air-profile.yaml

cargo run -p air-cli -- run-plan --profile examples/code-agent/repair-core.air-profile.yaml --log
```

This profile also modifies `examples/code-agent/repair-fixture/math.js`; reset or restore the
fixture after manual runs.

Run the bounded multi-file repair fixture:

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/repair-multifile.air-profile.yaml

cargo run -p air-cli -- run-plan --profile examples/code-agent/repair-multifile.air-profile.yaml --log
```

This uses the same core repair recipe, but `tools.core.json` allows a patch touching at most two
existing files. The fixture starts with failures split across `math.js` and `normalize.js`; the
verification gate asserts that the patch event reports `file_count == 2` and that the retest passes.

Deterministic verification for these examples:

```bash
bash scripts/verify_code_agent.sh
```

Run the offline code-agent bench harness:

```bash
bash scripts/bench_code_agent.sh
```

The bench writes route decisions and offline review/explore/repair outputs under
`target/generated/code-agent-bench/`, then emits `summary.json`. Set
`AIR_CODE_AGENT_BENCH_REAL_BUILD=1` to include the real-model page build path.

Set `AIR_CODE_AGENT_REAL=1` to include the real-model repair smoke; the script restores the repair
fixture afterward.
