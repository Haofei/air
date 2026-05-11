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
assert tools == ["file.search", "test.run", "file.read", "file.ops", "test.run", "git.diff"], tools
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
auto_verify_passed_index = next(
    index for index, event in enumerate(events)
    if event.get("rule") == "record-auto-verify-passed"
    and event.get("action") == "append"
)
summarizer_index = next(
    index for index, event in enumerate(events)
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_summarizer"
)
assert not any(
    event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
    for event in events[auto_verify_passed_index:summarizer_index]
), events[auto_verify_passed_index:summarizer_index]
assert any(
    event.get("rule") == "record-auto-verify-passed"
    and event.get("action") == "append"
    and event.get("output", [{}])[-1].get("action") == "final_diff_observed"
    for event in events
), events
decider_start = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
)
tool_schemas = decider_start["input"]["tool_schemas"]
assert "repo.files" in tool_schemas, tool_schemas
assert "file.ops" in tool_schemas, tool_schemas
assert "candidate.validate" in tool_schemas, tool_schemas
assert "test.run" in tool_schemas, tool_schemas
assert "pattern" in tool_schemas["repo.files"]["optional"], tool_schemas["repo.files"]
assert "max_changed_lines" in tool_schemas["file.ops"]["optional"], tool_schemas["file.ops"]
assert "max_changed_lines" in tool_schemas["file.patch"]["optional"], tool_schemas["file.patch"]
assert "candidate" in tool_schemas["candidate.validate"]["required"], tool_schemas["candidate.validate"]
assert "package" in tool_schemas["test.run"]["optional"], tool_schemas["test.run"]
assert "test_filter" in tool_schemas["test.run"]["optional"], tool_schemas["test.run"]
PY

echo "[code-agent] edit loop searches truncated manual test logs"
"${PYTHON:-python3}" - <<'PY'
import json

with open("examples/code-agent/tools.core.json", encoding="utf-8") as handle:
    config = json.load(handle)
config["tools"]["test.run"]["max_bytes"] = 8
config["tools"]["test.run"]["truncation_direction"] = "tail"
with open("target/generated/tools.truncated-manual-test.json", "w", encoding="utf-8") as handle:
    json.dump(config, handle, indent=2)
PY
edit_truncated_manual_test_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_truncated_manual_test_backup"
restore_edit_truncated_manual_test_fixture() {
  cp "$edit_truncated_manual_test_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_truncated_manual_test_backup"
}
trap restore_edit_truncated_manual_test_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config target/generated/tools.truncated-manual-test.json \
  --trace-out target/generated/code_agent_edit_truncated_manual_test.trace.jsonl \
  > target/generated/code_agent_edit_truncated_manual_test.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_truncated_manual_test.post_test.log
restore_edit_truncated_manual_test_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_truncated_manual_test.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_truncated_manual_test.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
manual_failed = next(
    event for event in events
    if event.get("rule") == "record-manual-test-failed-output-truncated"
    and event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.search"
)
assert manual_failed["input"]["path"], manual_failed
assert manual_failed["output"]["match_count"] >= 1, manual_failed
repair_decider = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
    and any(
        observation.get("action") == "manual_test_full_log_context"
        for observation in event.get("input", {}).get("observations", [])
    )
)
assert repair_decider["input"]["verification_status"] == "failed", repair_decider
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
    "git.diff",
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

echo "[code-agent] edit loop records path error hints"
edit_path_error_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_path_error_backup"
restore_edit_path_error_fixture() {
  cp "$edit_path_error_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_path_error_backup"
}
trap restore_edit_path_error_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.path-error.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_path_error.trace.jsonl \
  > target/generated/code_agent_edit_path_error.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_path_error.post_test.log
restore_edit_path_error_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_path_error.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_path_error.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
assert any(
    event.get("rule") == "record-file-read-path-error-hint"
    and event.get("action") == "append"
    and event.get("output", [{}])[-1].get("action") == "path_error_hint"
    and event.get("output", [{}])[-1].get("result", {}).get("target_path") == "examples/code-agent/edit-fixture/math.js"
    for event in events
), events
assert any(
    event.get("rule") == "preflight-target-search"
    and event.get("action") == "append"
    and event.get("output", [{}])[-1].get("action") == "initial_target_search"
    for event in events
), events
failed_read = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.read"
    and event.get("status") == "error"
)
assert failed_read["input"]["path"] == "examples/code-agent/edit-fixture/math.", failed_read
summarizer = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_summarizer"
)
assert any(
    observation.get("action") == "path_error_hint"
    for observation in summarizer["input"]["observations"]
), summarizer
PY

echo "[code-agent] edit loop records batch dispatch errors"
edit_batch_error_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_batch_error_backup"
restore_edit_batch_error_fixture() {
  cp "$edit_batch_error_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_batch_error_backup"
}
trap restore_edit_batch_error_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.batch-error.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_batch_error.trace.jsonl \
  > target/generated/code_agent_edit_batch_error.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_batch_error.post_test.log
restore_edit_batch_error_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_batch_error.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_batch_error.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
assert any(
    event.get("rule") == "record-batch-dispatch-error"
    and event.get("action") == "append"
    and event.get("output", [{}])[-1].get("action") == "batch_dispatch_error"
    and "write tool alone" in event.get("output", [{}])[-1].get("rationale", "")
    for event in events
), events
batch_error = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch"
    and event.get("meta", {}).get("error_count") == 1
    and event.get("output", [{}])[0].get("tool") == "<batch>"
)
assert "approval-required capability file.write must be isolated" in batch_error["output"][0]["error"], batch_error
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == ["file.search", "file.ops", "test.run", "git.diff"], tools
PY

echo "[code-agent] edit loop gives structured feedback for content wrappers"
edit_content_wrapper_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_content_wrapper_backup"
restore_edit_content_wrapper_fixture() {
  cp "$edit_content_wrapper_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_content_wrapper_backup"
}
trap restore_edit_content_wrapper_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.content-wrapper.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_content_wrapper.trace.jsonl \
  > target/generated/code_agent_edit_content_wrapper.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_content_wrapper.post_test.log
restore_edit_content_wrapper_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_content_wrapper.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_content_wrapper.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
schema_errors = [
    event for event in events
    if event.get("action") == "model_call"
    and event.get("status") == "error"
    and event.get("rule") == "choose"
]
assert schema_errors, events
error = schema_errors[0]["error"]
assert "invalid structured model output" in error, error
assert "declared AIR output interface" in error, error
assert "content_preview=" in error, error
assert schema_errors[0].get("meta", {}).get("will_retry") is True, schema_errors[0]
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == ["file.search", "file.ops", "test.run", "git.diff"], tools
PY

echo "[code-agent] edit loop records edit validation failures"
edit_validation_failed_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_validation_failed_backup"
restore_edit_validation_failed_fixture() {
  cp "$edit_validation_failed_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_validation_failed_backup"
}
trap restore_edit_validation_failed_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.validation-failed.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_validation_failed.trace.jsonl \
  > target/generated/code_agent_edit_validation_failed.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_validation_failed.post_test.log
restore_edit_validation_failed_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_validation_failed.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_validation_failed.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
assert any(
    event.get("rule") == "record-file-ops-validation-failed"
    and event.get("action") == "append"
    and event.get("output", [{}])[-1].get("action") == "edit_validation_failed"
    and "prefer kind=replace_lines" in event.get("output", [{}])[-1].get("rationale", "")
    for event in events
), events
failed_file_ops = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.ops"
    and event.get("status") == "ok"
    and event.get("output", {}).get("applied") is False
)
assert failed_file_ops["output"]["diagnostics"][0]["field"] == "old_string", failed_file_ops
successful_file_ops = [
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.ops"
    and event.get("status") == "ok"
    and event.get("output", {}).get("applied") is True
]
assert successful_file_ops, events
assert successful_file_ops[0]["input"]["operations"][0]["kind"] == "replace_lines", successful_file_ops[0]
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == ["file.search", "file.ops", "diagnostic.context", "file.ops", "test.run", "git.diff"], tools
diagnostic_context = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "diagnostic.context"
)
repair_decider = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
    and any(
        observation.get("action") == "edit_validation_context"
        for observation in event.get("input", {}).get("observations", [])
    )
)
assert diagnostic_context["input"]["diagnostics"], diagnostic_context
assert diagnostic_context["input"]["diagnostics"][0]["line"] == 2, diagnostic_context
assert diagnostic_context["input"]["diagnostics"][0]["line_source"] == "old_string_anchor", diagnostic_context
assert diagnostic_context["output"]["snippets"], diagnostic_context
assert "examples/code-agent/edit-fixture/math.js" in diagnostic_context["output"]["snippets"][0]["path"], diagnostic_context
assert diagnostic_context["output"]["snippets"][0]["start_line"] == 1, diagnostic_context
assert diagnostic_context["output"]["snippets"][0]["end_line"] == 5, diagnostic_context
assert diagnostic_context["output"]["snippets"][0]["path_only_diagnostic_indexes"] == [], diagnostic_context
assert repair_decider["input"]["verification_status"] == "unknown", repair_decider
PY

echo "[code-agent] edit loop observes candidate validation errors"
edit_candidate_validate_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_candidate_validate_backup"
restore_edit_candidate_validate_fixture() {
  cp "$edit_candidate_validate_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_candidate_validate_backup"
}
trap restore_edit_candidate_validate_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.candidate-validate.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_candidate_validate.trace.jsonl \
  > target/generated/code_agent_edit_candidate_validate.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_candidate_validate.post_test.log
restore_edit_candidate_validate_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_candidate_validate.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_candidate_validate.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
candidate_error = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "candidate.validate"
    and event.get("status") == "error"
)
candidate_ok = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "candidate.validate"
    and event.get("status") == "ok"
)
decider_after_error = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
    and any(
        result.get("tool") == "candidate.validate" and result.get("status") == "error"
        for observation in event.get("input", {}).get("observations", [])
        for result in observation.get("result", [])
    )
)
assert "not_allowlisted" in candidate_error["error"], candidate_error
assert candidate_ok["output"]["valid"] is True, candidate_ok
assert candidate_ok["output"]["test_command"] == "edit_fixture_test", candidate_ok
assert decider_after_error["input"]["verification_status"] == "unknown", decider_after_error
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == ["file.search", "candidate.validate", "file.read", "file.ops", "test.run", "git.diff"], tools
PY

echo "[code-agent] edit loop collects diagnostic context after failed auto verify"
edit_auto_verify_failed_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_auto_verify_failed_backup"
restore_edit_auto_verify_failed_fixture() {
  cp "$edit_auto_verify_failed_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_auto_verify_failed_backup"
}
trap restore_edit_auto_verify_failed_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.auto-verify-failed.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_auto_verify_failed.trace.jsonl \
  > target/generated/code_agent_edit_auto_verify_failed.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_auto_verify_failed.post_test.log
restore_edit_auto_verify_failed_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_auto_verify_failed.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_auto_verify_failed.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == [
    "file.search",
    "file.ops",
    "test.run",
    "diagnostic.context",
    "file.ops",
    "test.run",
    "git.diff",
], tools
failed_verify = next(
    event for event in events
    if event.get("rule") == "record-auto-verify-failed-output"
    and event.get("action") == "append"
    and event.get("output", [{}])[-1].get("action") == "auto_verify_failed"
)
diagnostic_context = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "diagnostic.context"
)
repair_decider = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
    and any(
        observation.get("action") == "auto_verify_diagnostic_context"
        for observation in event.get("input", {}).get("observations", [])
    )
)
assert failed_verify["input"]["result"][0]["output"]["success"] is False, failed_verify
assert diagnostic_context["input"]["diagnostics"], diagnostic_context
assert diagnostic_context["output"]["snippets"], diagnostic_context
assert "examples/code-agent/edit-fixture/math.js" in diagnostic_context["output"]["snippets"][0]["path"], diagnostic_context
assert repair_decider["input"]["verification_status"] == "failed", repair_decider
PY

echo "[code-agent] edit loop searches truncated auto verify logs"
"${PYTHON:-python3}" - <<'PY'
import json

with open("examples/code-agent/tools.core.json", encoding="utf-8") as handle:
    config = json.load(handle)
config["tools"]["test.run"]["max_bytes"] = 8
config["tools"]["test.run"]["truncation_direction"] = "tail"
with open("target/generated/tools.truncated-auto-verify.json", "w", encoding="utf-8") as handle:
    json.dump(config, handle, indent=2)
PY
edit_truncated_auto_verify_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_truncated_auto_verify_backup"
restore_edit_truncated_auto_verify_fixture() {
  cp "$edit_truncated_auto_verify_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_truncated_auto_verify_backup"
}
trap restore_edit_truncated_auto_verify_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.auto-verify-failed.json \
  --tool-config target/generated/tools.truncated-auto-verify.json \
  --trace-out target/generated/code_agent_edit_truncated_auto_verify.trace.jsonl \
  > target/generated/code_agent_edit_truncated_auto_verify.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_truncated_auto_verify.post_test.log
restore_edit_truncated_auto_verify_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_truncated_auto_verify.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_truncated_auto_verify.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
failed_verify = next(
    event for event in events
    if event.get("rule") == "record-auto-verify-failed-output-truncated"
    and event.get("action") == "append"
    and event.get("output", [{}])[-1].get("action") == "auto_verify_failed"
)
assert failed_verify["input"]["result"][0]["output"]["truncated"] is True, failed_verify
assert failed_verify["input"]["result"][0]["output"]["full_log_path"], failed_verify
full_log_search = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.search"
    and event.get("input", {}).get("path") == failed_verify["input"]["result"][0]["output"]["full_log_path"]
)
assert full_log_search["output"]["match_count"] >= 1, full_log_search
repair_decider = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
    and any(
        observation.get("action") == "auto_verify_full_log_context"
        for observation in event.get("input", {}).get("observations", [])
    )
)
assert repair_decider["input"]["verification_status"] == "failed", repair_decider
PY

echo "[code-agent] edit loop repairs single allowed write path"
edit_write_path_error_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_write_path_error_backup"
restore_edit_write_path_error_fixture() {
  cp "$edit_write_path_error_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_write_path_error_backup"
}
trap restore_edit_write_path_error_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.write-path-error.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_write_path_repair.trace.jsonl \
  > target/generated/code_agent_edit_write_path_repair.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_write_path_repair.post_test.log
restore_edit_write_path_error_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_write_path_repair.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_write_path_repair.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
file_ops = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.ops"
    and event.get("status") == "ok"
)
assert file_ops["output"]["path_repairs"][0]["from"] == "examples/code-agent/edit-fixture/math.", file_ops
assert file_ops["output"]["path_repairs"][0]["to"] == "examples/code-agent/edit-fixture/math.js", file_ops
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == ["file.search", "file.ops", "test.run", "git.diff"], tools
summarizer = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_summarizer"
)
assert any(
    result.get("output", {}).get("path_repairs")
    for observation in summarizer["input"]["observations"]
    for result in observation.get("result", [])
), summarizer
PY

echo "[code-agent] edit loop blocks premature completion"
edit_premature_backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$edit_premature_backup"
restore_edit_premature_fixture() {
  cp "$edit_premature_backup" examples/code-agent/edit-fixture/math.js
  rm -f "$edit_premature_backup"
}
trap restore_edit_premature_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/edit.input.json \
  --model-config examples/code-agent/model-fixtures.premature-complete.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_edit_premature.trace.jsonl \
  > target/generated/code_agent_edit_premature.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_premature.post_test.log
restore_edit_premature_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_premature.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code_agent_edit_premature.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
assert any(
    event.get("rule") == "block-complete-without-passed-verification"
    and event.get("action") == "append"
    and event.get("output", [{}])[-1].get("action") == "completion_blocked"
    for event in events
), events
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert tools == ["file.search", "test.run", "file.read", "file.ops", "test.run", "git.diff"], tools
summarizer = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_summarizer"
)
assert any(
    observation.get("action") == "completion_blocked"
    for observation in summarizer["input"]["observations"]
), summarizer
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
assert tools == ["file.search", "test.run", "file.read", "file.patch", "test.run", "git.diff"], tools
file_patch = next(
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "file.patch"
)
assert file_patch["input"]["allowed_paths"] == ["examples/code-agent/edit-fixture/math.js"], file_patch
assert any(event.get("action") == "approval" and event.get("status") == "ok" for event in events), events
PY

echo "[code-agent] ok"
