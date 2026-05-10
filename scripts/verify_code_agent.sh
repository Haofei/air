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
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/edit.self.air-profile.yaml

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

echo "[code-agent] default explore profile run"
cargo run -q -p air-cli -- code "check whether build is a public code-agent primitive" \
  --recipe explore \
  --query "build code agent recipe primitive" \
  --trace-out target/generated/code_agent_explore.trace.jsonl \
  > target/generated/code_agent_explore.output.json

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_explore.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
exploration = output["exploration"]
assert isinstance(exploration["summary"], str) and exploration["summary"], exploration
assert isinstance(exploration["relevant_files"], list), exploration
assert isinstance(exploration["findings"], list), exploration

with open("target/generated/code_agent_explore.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
assert any(event.get("rule") == "skip-target-read" for event in events), events
PY

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
file_ops = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.ops"
)
assert file_ops["input"]["allowed_paths"] == ["examples/code-agent/edit-fixture/math.js"], file_ops
assert any(event.get("action") == "tool_batch_dispatch" for event in events), events
assert any(
    event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "error"
    and event.get("meta", {}).get("tool") == "<invalid>"
    for event in events
), events
assert any(
    event.get("action") == "tool_batch_dispatch"
    and event.get("meta", {}).get("error_count") == 1
    for event in events
), events
models = [
    event.get("meta", {}).get("model")
    for event in events
    if event.get("action") == "model_call"
]
assert "code_edit_decider" in models, models
assert "code_edit_summarizer" in models, models
decider_start = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
)
tool_schemas = decider_start["input"]["tool_schemas"]
assert "repo.files" in tool_schemas, tool_schemas
assert "file.ops" in tool_schemas, tool_schemas
assert "test.run" in tool_schemas, tool_schemas
assert "pattern" in tool_schemas["repo.files"]["optional"], tool_schemas["repo.files"]
assert "max_changed_lines" in tool_schemas["file.ops"]["optional"], tool_schemas["file.ops"]
assert "max_changed_lines" in tool_schemas["file.patch"]["optional"], tool_schemas["file.patch"]
assert "package" in tool_schemas["test.run"]["optional"], tool_schemas["test.run"]
assert "test_filter" in tool_schemas["test.run"]["optional"], tool_schemas["test.run"]
PY

echo "[code-agent] targetless edit loop offline run"
edit_targetless_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_targetless_backup"
restore_edit_targetless_fixture() {
  cp "$edit_targetless_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_targetless_backup"
}
trap restore_edit_targetless_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.targetless.input.json \
  --model-config examples/code-agent/model-fixtures.targetless.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_targetless.trace.jsonl \
  > target/generated/code_agent_edit_targetless.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_targetless.post_test.log
restore_edit_targetless_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_targetless.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["initial_success"] is False, edit
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit
assert "examples/code-agent/edit-fixture/math.js" in edit["workspace_diff"], edit

with open("target/generated/code_agent_edit_targetless.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == [
    "test.run",
    "repo.search",
    "diagnostic.context",
    "file.search",
    "file.read_many",
    "file.ops",
    "test.run",
], tools
repo_search = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "repo.search"
)
assert repo_search["output"]["query_source"] == "pattern", repo_search
assert repo_search["output"]["glob"] == "examples/code-agent/edit-fixture/*.js", repo_search
diagnostics = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "diagnostic.context"
)
assert diagnostics["output"]["diagnostic_count"] == 0, diagnostics
file_search = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.search"
)
assert file_search["output"]["directory"] is True, file_search
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

echo "[code-agent] user-facing self edit command explain"
cargo run -q -p air-cli -- code "update AIR code-agent docs and run the code-agent gate" \
  --recipe edit \
  --profile examples/code-agent/edit.self.air-profile.yaml \
  --target examples/code-agent/README.md \
  --test verify_code_agent \
  --explain \
  > target/generated/code_agent_edit_self_explain.output.json

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_self_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
assert output["resolved_recipe"] == "edit", output
assert output["profile"] == "examples/code-agent/edit.self.air-profile.yaml", output
assert output["pack"]["recipe"] == "edit", output
assert output["pack"]["default_profile"] == "examples/code-agent/edit.air-profile.yaml", output
assert output["pack"]["profile_override"] is True, output
assert output["input"]["target_path"] == "examples/code-agent/README.md", output
assert output["input"]["test_command"] == "verify_code_agent", output
PY

echo "[code-agent] edit loop patch run"
edit_patch_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_patch_backup"
restore_edit_patch_fixture() {
  cp "$edit_patch_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_patch_backup"
}
trap restore_edit_patch_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.patch.input.json \
  --model-config examples/code-agent/model-fixtures.patch.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_patch.trace.jsonl \
  > target/generated/code_agent_edit_patch.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_patch.post_test.log
restore_edit_patch_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_patch.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit
assert "examples/code-agent/edit-fixture/math.js" in edit["workspace_diff"]["diff"], edit

with open("target/generated/code_agent_edit_patch.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == ["test.run", "file.read", "file.patch", "test.run", "git.diff"], tools
file_patch = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.patch"
)
assert file_patch["input"]["allowed_paths"] == ["examples/code-agent/edit-fixture/math.js"], file_patch
assert any(event.get("action") == "approval" and event.get("status") == "ok" for event in events), events
PY

echo "[code-agent] ok"
