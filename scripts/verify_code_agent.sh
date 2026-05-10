#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p target/generated

echo "[code-agent] validate current primitive recipes"
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/project-plan.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/explore.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/dynamic-explore.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/profile.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/edit.air-profile.yaml

echo "[code-agent] pack and model schema tests"
cargo test -q -p air-cli code_agent_pack_declares_all_default_profiles
cargo test -q -p air-cli pack_validation_rejects
cargo test -q -p air-cli pack_input_contract
cargo test -q -p air-cli pack_auto_routing
cargo test -q -p air-cli model_config_prompts_match_code_agent_schemas

echo "[code-agent] deterministic tool coverage"
node --check scripts/playwright_search.cjs
node --check scripts/playwright_search_fixture_test.cjs
node --check scripts/playwright_page_audit.cjs
node --check scripts/playwright_page_audit_fixture_test.cjs
node scripts/playwright_search_fixture_test.cjs
node scripts/playwright_page_audit_fixture_test.cjs
cargo test -q -p air-tools file_read
cargo test -q -p air-tools file_search
cargo test -q -p air-tools file_ops
cargo test -q -p air-tools file_patch
cargo test -q -p air-tools git_status
cargo test -q -p air-tools command_run
cargo test -q -p air-tools repo_search
cargo test -q -p air-tools repo_context
cargo test -q -p air-tools diagnostic_context

echo "[code-agent] edit loop offline run"
edit_fixture_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_fixture_backup"
restore_edit_fixture() {
  cp "$edit_fixture_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_fixture_backup"
}
trap restore_edit_fixture EXIT
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/edit.air-profile.yaml \
  --trace-out target/generated/code_agent_edit.trace.jsonl \
  > target/generated/code_agent_edit.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit.post_test.log
restore_edit_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["initial_success"] is False, edit
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit
assert "examples/code-agent/edit-fixture/math.js" in edit["workspace_diff"]["diff"], edit

with open("target/generated/code_agent_edit.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == ["test.run", "file.read", "file.ops", "test.run", "git.diff"], tools
assert any(event.get("action") == "tool_batch_dispatch" for event in events), events
models = [
    event.get("meta", {}).get("model")
    for event in events
    if event.get("action") == "model_call"
]
assert "code_edit_decider" in models, models
assert "code_edit_summarizer" in models, models
PY

echo "[code-agent] user-facing edit command explain"
cargo run -q -p air-cli -- code "edit the failing add function and retest" \
  --recipe edit \
  --target examples/code-agent/edit-fixture/math.js \
  --test edit_fixture_test \
  --related examples/code-agent/edit-fixture/test.js \
  --explain \
  > target/generated/code_agent_edit_explain.output.json

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
assert output["resolved_recipe"] == "edit", output
assert output["pack"]["recipe"] == "edit", output
assert output["pack"]["default_profile"] == "examples/code-agent/edit.air-profile.yaml", output
assert output["input"]["test_command"] == "edit_fixture_test", output
PY

echo "[code-agent] ok"
