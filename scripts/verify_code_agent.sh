#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p target/generated

echo "[code-agent] validate packaged profiles"
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/profile.air-profile.yaml
cargo run -q -p air-cli -- validate-plan examples/code-agent/code-review.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml
cargo run -q -p air-cli -- validate-plan examples/code-agent/code-review-composed.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/apple-build.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/repair.air-profile.yaml

echo "[code-agent] deterministic tool coverage"
node --check scripts/playwright_search.cjs
node --check scripts/playwright_search_fixture_test.cjs
node --check scripts/playwright_page_audit.cjs
node --check scripts/playwright_page_audit_fixture_test.cjs
node scripts/playwright_search_fixture_test.cjs
node scripts/playwright_page_audit_fixture_test.cjs
cargo test -q -p air-tools file_read
cargo test -q -p air-tools file_read_many
cargo test -q -p air-tools file_patch
cargo test -q -p air-tools file_edit
cargo test -q -p air-tools stale_read
cargo test -q -p air-tools git_status
cargo test -q -p air-tools todo_write
cargo test -q -p air-tools todo_read
cargo test -q -p air-tools context_measure
cargo test -q -p air-tools playwright_page_audit
cargo test -q -p air-tools repo_search
cargo test -q -p air-tools repo_symbols
cargo test -q -p air-tools repo_references
cargo test -q -p air-tools repo_context
cargo test -q -p air-tools diagnostic_context
cargo test -q -p air-tools command_run_extracts_structured_diagnostics
cargo test -q -p air-tools extract_command_diagnostics_parses_rustc_location_blocks
cargo test -q -p air-tools extract_command_diagnostics_parses_python_tracebacks
cargo test -q -p air-tools extract_command_diagnostics_parses_line_only_colon_diagnostics
cargo test -q -p air-tools extract_command_diagnostics_parses_file_context_lint_blocks

echo "[code-agent] composed review offline run"
cargo run -q -p air-cli -- run-plan examples/code-agent/code-review-composed.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/input.json \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  > target/generated/code_review_composed_fixture.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_review_composed_fixture.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
review = output["review"]
assert review["summary"].startswith("Fixture review completed")
assert review["findings"][0]["severity"] == "info"
assert review["search_quality"]["sufficient"] is True
PY

echo "[code-agent] repair fixture starts failing with a structured diagnostic"
if node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_repair_fixture.log 2>&1; then
  echo "repair fixture unexpectedly passed; it should start from a failing implementation" >&2
  cat target/generated/code_agent_repair_fixture.log >&2
  exit 1
fi
grep -q "examples/code-agent/repair-fixture/math.js:2:10: error:" \
  target/generated/code_agent_repair_fixture.log

if [[ "${AIR_CODE_AGENT_REAL:-0}" == "1" ]]; then
  echo "[code-agent] real repair smoke"
  original="$(cat examples/code-agent/repair-fixture/math.js)"
  restore_fixture() {
    printf "%s" "$original" > examples/code-agent/repair-fixture/math.js
  }
  trap restore_fixture EXIT

  cargo run -q -p air-cli -- run-plan --profile examples/code-agent/repair.air-profile.yaml --log \
    > target/generated/code_agent_repair_real.output.json
  node examples/code-agent/repair-fixture/test.js
  restore_fixture
  trap - EXIT
fi

echo "[code-agent] verification complete"
