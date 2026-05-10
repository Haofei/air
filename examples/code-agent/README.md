# AIR Code Agent

This example keeps the coding agent close to AIR's own design rule: small primitives, explicit policy, and auditable traces.

The public recipes are:

- `plan`: turn an open coding goal into bounded tasks, files, dependencies, and acceptance checks.
- `explore`: read-only repository exploration.
- `review`: grounded review over repository, diff, test, and optional search evidence.
- `edit`: the only workspace-writing primitive. The model chooses one declared tool per turn; AIR executes it, appends the observation, enforces capability/budget policy, and records the trace.

`edit` covers bug fixes, behavior-preserving changes, small feature edits, and bounded file creation. Those are task intents, not separate agent primitives.

## Edit Loop

The edit module is `code-edit-loop.air.yaml`, wired by `code-edit.air-plan.yaml` and `edit.air-profile.yaml`.

It runs as:

```text
init -> choose -> tool_dispatch -> choose -> ... -> summarize -> done
```

The fixed structure is only the loop boundary. The model must choose each read/search/edit/test/diff step through declared tools such as `file.read`, `file.search`, `file.ops`, `test.run`, and `git.diff`.

Run the deterministic fixture:

```bash
cargo run -p air-cli -- validate-plan --profile examples/code-agent/edit.air-profile.yaml
cargo run -p air-cli -- run-plan --profile examples/code-agent/edit.air-profile.yaml --log
```

Run through the user-facing wrapper:

```bash
cargo run -p air-cli -- code "edit the failing add function and retest" \
  --recipe edit \
  --target examples/code-agent/edit-fixture/math.js \
  --test edit_fixture_test \
  --related examples/code-agent/edit-fixture/test.js
```

## Verification

```bash
bash scripts/verify_code_agent.sh
```

The gate validates current profiles, tool coverage, pack routing, prompt/schema alignment, and the edit loop trace shape.
