# AIR Code Agent

This example keeps the coding agent close to AIR's own design rule: small primitives, explicit policy, and auditable traces.

## Development Rule

Code-agent work follows opencode-style practices as a standing requirement, not as an optional
comparison point:

- Start from a real or deterministic failing run, preferably with a trace, before adding behavior.
- Check how opencode handles the same class of problem when the issue is tool feedback, repeated
  calls, model formatting variance, context growth, step limits, or edit verification.
- Add the smallest AIR primitive or tool behavior that preserves the generic IR. Do not add
  task-shaped agent modes such as separate repair/refactor/build primitives.
- Use TDD: add the failing fixture or contract test first, then implement, then run the code-agent
  gate.
- Keep the final evidence inspectable: trace stats, model/tool call sequence, verification command,
  and any remaining limitation should be visible from the run output or checked artifacts.

The public recipes are:

- `explore`: read-only repository exploration.
- `review`: grounded review over repository, diff, test, and optional search evidence.
- `edit`: the only workspace-writing primitive. The model chooses one declared tool per turn; AIR executes it, appends the observation, enforces capability/budget policy, and records the trace.

`edit` covers bug fixes, behavior-preserving changes, small feature edits, and bounded file creation. Those are task intents, not separate agent primitives. A target file is useful but optional for explicit edit runs; when omitted, the loop must discover and read the file before any write.

Top-level files are the public entrypoints. `fixtures/` contains deterministic model configs, alternate tool configs, and internal verification variants used by tests and dogfood scripts.

## Explore

`explore` is read-only and can start without a known target file. When no `--target` is supplied, AIR passes an empty `target_path`; the module skips direct file read and relies on repository search, symbol lookup, and context snippets to find the relevant entry points.

```bash
cargo run -p air-cli -- code "explore the code-agent edit loop architecture" \
  --recipe explore \
  --query "code agent edit loop architecture"
```

## Edit Loop

The edit module is `code-edit-loop.air.yaml`, wired by `code-edit.air-plan.yaml` and `edit.air-profile.yaml`.

It runs as:

```text
init -> choose -> tool_batch_dispatch -> choose -> ... -> summarize -> done
```

The fixed structure is only the loop boundary. The model chooses one to four planning/discovery/read/search/edit/test/diff tool calls per turn through declared tools such as `todo.write`, `todo.read`, `repo.files`, `repo.search`, `repo.symbols`, `repo.references`, `lsp.references`, `lsp.diagnostics`, `file.read`, `file.search`, `file.ops`, `code.assert`, `test.run`, and `git.diff`.

`tools.self.json` is the stricter AIR dogfood tool config. It keeps shell access behind fixed formatter and verification commands, so the model can ask for `format.run` or `test.run` without choosing a command variant. The default self-validation runs workspace tests and clippy, matching the CI failure modes that matter for AIR changes.

Completion is blocked until formatting and verification have passed. After a `file.ops` write, AIR automatically runs `format.run` and then `test.run`; the model does not choose formatter timing. The loop does not transition to `summarize -> done` until verification returns `success: true`. If formatting or tests fail, the model must continue iterating — reading diagnostics, adjusting code, and re-testing — before the loop can finish.

The default edit budget is sized for real bounded coding work rather than a smoke test: `max_steps: 160`, `max_model_calls: 40`, `max_tool_calls: 200`, `max_repeated_tool_calls: 16`, and a 30-observation / 200KB model input window. This gives the loop room for repeated explore -> edit -> verify -> fix cycles while AIR still enforces approval, tool, and trace boundaries.

The edit loop receives the current step budget through the `_air` runtime context. When the state-machine step budget is nearly exhausted, the model summarizes instead of choosing more tools, ensuring the loop closes cleanly within its allocated budget.

When `test.run` output is truncated (`output.truncated == true`), the full log is written to the path reported in `output.full_log_path`. Use `file.search` or a narrow `file.read` range on that path to inspect hidden lines instead of re-running only to recover truncated output.

When `file.ops` `edit` validation fails (e.g. `old_string` not found), the AIR edit loop automatically collects `diagnostic.context` before the next decision so the model sees the surrounding lines and can correct the anchor. If the full `old_string` does not match but one of its lines is present, the diagnostic includes an `old_string_anchor` line for a precise retry location.

The edit loop runs `candidate.validate` automatically during preflight when `target_path` is known, catching misconfigured paths before any edit attempt.
Use `code.assert` for structural postconditions that tests may not prove directly, such as `symbol_absent`, `symbol_present`, `file_contains`, or `file_not_contains`.

Use `repo.symbols` for cheap symbol ranges and `lsp.references` when semantic references matter, then make explicit `file.ops` edits and let validation diagnostics close the loop.

Use `lsp.references` before larger refactors when regex references are too weak. The current bundled implementation uses rust-analyzer and keeps that LSP session alive inside the tool provider for the duration of the run, so repeated semantic lookups do not restart the language server. `lsp.diagnostics` exposes language-server diagnostics for the next repair step.

Use `edit.self.air-profile.yaml` when dogfooding AIR itself with a real OpenAI-compatible model:

```bash
cargo run -p air-cli -- code "update the AIR code-agent docs and run the code-agent gate" \
  --recipe edit \
  --profile examples/code-agent/edit.self.air-profile.yaml \
  --target examples/code-agent/README.md
```

For targetless edit, let the loop discover the file and run the configured validation:

```bash
cargo run -p air-cli -- code "fix the failing add function" \
  --recipe edit \
  --query "edit fixture add function test"
```

Run the deterministic fixture:

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/edit.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/code-agent/edit.air-profile.yaml --log
```

## Review

Run the default review profile:

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/review.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/code-agent/review.air-profile.yaml --log
```

The default review example stays simple: one bounded review module, deterministic local search tools, and a single profile. Browser-backed search and composed review variants remain under `fixtures/` for internal verification.

Run through the user-facing wrapper:

```bash
cargo run -p air-cli -- code "edit the failing add function and retest" \
  --recipe edit \
  --target examples/code-agent/edit-fixture/math.js \
  --related examples/code-agent/edit-fixture/test.js
```

## Verification

```bash
bash scripts/verify_code_agent.sh
```

The gate validates current profiles, tool coverage, pack routing, prompt/schema alignment, and the edit loop trace shape.
