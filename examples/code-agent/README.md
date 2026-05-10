# Code Agent Example

This example contains two bounded AIR coding agents:

- `code.review@0.1.0` gathers evidence and returns review guidance that cites concrete artifacts.
- `code.repair@0.1.0` reads a target file, runs an allowlisted test, uses structured diagnostics to
  generate a unified diff, validates it with `file.patch` dry-run, applies it through constrained
  `file.patch`, then retests with one bounded retry pass if the first patch does not fix the test.
  It finishes by calling `git.status` so the returned summary includes workspace cleanliness and
  changed files.
- `code.build_page@0.1.0` generates one static HTML file, writes it through constrained `file.write`,
  then runs an allowlisted smoke test.

The review agent uses:

- `todo.write` for a structured progress artifact before evidence gathering;
- `web.search` for external documentation or issues;
- `repo.files` for relevant repository paths;
- `repo.search` for symbol or text matches, with explicit regex mode available for grep-style discovery;
- `repo.context` for automatically selected nearby code snippets around repository matches;
- `file.read` for the target source file, with optional numbered output for diagnostics;
- `git.diff` for local changes to that file.
- `git.status` for structured workspace change awareness.
- `test.run` for an allowlisted verification command.

For editing agents, prefer the opencode-style tool split already available in `air-tools`:
`todo.write` for explicit task tracking on non-trivial work,
`file.read` for context, `file.edit` for exact-string changes that require a prior read by default
and explicit whitespace-tolerant strategies for indentation drift, with bounded diff output for
audit,
`file.patch` for reviewed multi-file unified diffs, and `file.write` for bounded file creation or
explicit overwrites. Keep shell execution behind `command_run` aliases instead of giving the model
a raw shell. `command_run` returns both raw logs and structured `diagnostics[]`, so repair loops can
focus on file/line/column errors instead of re-parsing terminal output from scratch.
When `require_read` is enabled, `file.write`, `file.edit`, and `file.patch` reject edits to files
that were not read or were modified after the last read. `file.patch` also supports `dry_run: true`
for `git apply --check` validation without mutating files; the repair agent uses that check before
every patch apply.

The default `tools.json` uses deterministic local search documents so release verification does
not depend on network access. For real research, switch to `tools.playwright.json`.

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/profile.air-profile.yaml

cargo run -p air-cli -- run-plan examples/code-agent/code-review.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/input.json \
  --model-config examples/bigmodel-openai-compatible.json \
  --tool-config examples/code-agent/tools.playwright.json \
  --trace-out target/generated/code_agent.trace.jsonl
```

Use this as the first coding-agent shape for bench work. It is intentionally static and bounded so
search quality, source grounding, and local-code evidence can be tested.

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

Deterministic verification for these examples:

```bash
bash scripts/verify_code_agent.sh
```

Set `AIR_CODE_AGENT_REAL=1` to include the real-model repair smoke; the script restores the repair
fixture afterward.
