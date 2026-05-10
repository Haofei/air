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
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/explore.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/dynamic-explore.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/apple-build.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/repair.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/repair-core.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/repair-multifile.air-profile.yaml

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
cargo test -q -p air-tools command_run
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
  --trace-out target/generated/code_review_composed_fixture.trace.jsonl \
  > target/generated/code_review_composed_fixture.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_review_composed_fixture.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
review = output["review"]
assert review["summary"].startswith("Fixture review completed")
assert review["findings"][0]["severity"] == "info"
assert review["search_quality"]["sufficient"] is True
with open("target/generated/code_review_composed_fixture.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
checks = [
    event for event in events
    if event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "artifact.validate"
]
assert checks, "expected artifact.validate trace event"
assert checks[-1]["output"]["valid"] is True, checks[-1]
PY

echo "[code-agent] user-facing review command offline run"
cargo run -q -p air-cli -- code "review the Playwright search tool" \
  --recipe review \
  --target scripts/playwright_search.cjs \
  --query playwright_search \
  --search-query "Playwright browser search result extraction timeout Node.js" \
  --required-term playwright \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  > target/generated/code_command_review.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_review.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
review = output["review"]
assert review["summary"].startswith("Fixture review completed")
assert review["search_quality"]["sufficient"] is True
PY

echo "[code-agent] user-facing code command explain"
cargo run -q -p air-cli -- code "fix the failing add function and retest" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --explain \
  > target/generated/code_command_explain.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
assert output["command"] == "code", output
assert output["will_run"] is False, output
assert output["requested_recipe"] == "auto", output
assert output["resolved_recipe"] == "repair", output
assert output["profile"] == "examples/code-agent/repair-core.air-profile.yaml", output
assert output["plan"].endswith("examples/code-agent/code-repair-with-explore.air-plan.yaml"), output
assert output["store"].endswith("examples/code-agent/module-store.air-store.yaml"), output
assert "file.write" in output["capabilities"], output
assert output["read_only"] is False, output
assert output["writes_workspace"] is True, output
assert output["budget"]["per_iteration"]["max_estimated_model_calls"] > 0, output
assert output["budget"]["per_iteration"]["max_estimated_tool_calls"] > 0, output
assert output["budget"]["total"]["iterations"] == 1, output
assert output["budget"]["total"]["max_estimated_tool_calls"] == output["budget"]["per_iteration"]["max_estimated_tool_calls"], output
assert output["loop"]["enabled"] is False, output
assert output["loop"]["max_iterations"] == 3, output
assert output["input"]["test_command"] == "repair_fixture_test", output
PY

cargo run -q -p air-cli -- code "fix the failing add function until tests pass" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --loop \
  --max-iterations 2 \
  --explain \
  > target/generated/code_command_loop_explain.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_loop_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)

per = output["budget"]["per_iteration"]
total = output["budget"]["total"]
assert output["loop"]["enabled"] is True, output
assert total["iterations"] == 2, output
assert total["max_estimated_model_calls"] == per["max_estimated_model_calls"] * 2, output
assert total["max_estimated_tool_calls"] == per["max_estimated_tool_calls"] * 2, output
PY

check_code_agent_route() {
  local name="$1"
  local task="$2"
  local expected_id="$3"
  local expected_source="$4"
  local output="target/generated/code_agent_plan_${name}.json"

  cargo run -q -p air-cli -- plan --explain \
    --store examples/code-agent/module-store.air-store.yaml \
    --task "$task" \
    > "$output"
  "${PYTHON:-python3}" - "$output" "$expected_id" "$expected_source" <<'PY'
import json
import sys

path, expected_id, expected_source = sys.argv[1:4]
with open(path, encoding="utf-8") as handle:
    output = json.load(handle)
first = output["component_selection"]["first_choice"]
assert first["id"] == expected_id, first
assert first["source"] == expected_source, first
assert first["tier"] == "large_component"
for candidate in output["component_selection"]["recommended"]:
    assert candidate["matched_terms"], candidate
PY
}

echo "[code-agent] primary component routing explain"
check_code_agent_route \
  "review" \
  "Review the command_run implementation for safety, provenance, and diagnostics." \
  "code.review_with_std_context@0.1.0" \
  "recipe"
check_code_agent_route \
  "repair" \
  "Fix a failing test using structured diagnostics, apply a bounded patch, and retest." \
  "code.core_repair@0.1.0" \
  "recipe"
check_code_agent_route \
  "build" \
  "Build an Apple-style landing page as a single HTML file and verify screenshots." \
  "code.build_page@0.1.0" \
  "module"
check_code_agent_route \
  "explore" \
  "Explore how command_run is implemented and identify relevant repository files." \
  "code.explore@0.1.0" \
  "module"
check_code_agent_route \
  "dynamic_explore" \
  "Explore the repository with a dynamic plan-act-observe loop that lets the model choose declared read-only tools." \
  "code.dynamic_explore@0.1.0" \
  "module"

echo "[code-agent] read-only explore offline run"
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/explore.air-profile.yaml \
  > target/generated/code_explore_fixture.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_explore_fixture.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
exploration = output["exploration"]
assert exploration["summary"].startswith("Fixture exploration completed")
assert any(item["path"] == "crates/air-tools/src/lib.rs" for item in exploration["relevant_files"])
assert exploration["findings"][0]["source_ids"]
PY

echo "[code-agent] dynamic read-only explore offline run"
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/dynamic-explore.air-profile.yaml \
  --trace-out target/generated/code_dynamic_explore_fixture.trace.jsonl \
  > target/generated/code_dynamic_explore_fixture.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_dynamic_explore_fixture.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
exploration = output["exploration"]
assert exploration["summary"].startswith("Fixture dynamic exploration completed")
assert any(item["path"] == "crates/air-runtime/src/lib.rs" for item in exploration["relevant_files"])

with open("target/generated/code_dynamic_explore_fixture.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
dispatches = [
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") in {
        "repo.files",
        "repo.search",
        "repo.symbols",
        "repo.references",
        "repo.context",
        "file.read_many",
    }
]
seen_tools = {event.get("meta", {}).get("tool") for event in dispatches}
assert {
    "repo.files",
    "repo.search",
    "repo.symbols",
    "repo.references",
    "repo.context",
    "file.read_many",
} <= seen_tools, seen_tools
batch_done = [
    event for event in events
    if event.get("action") == "tool_batch_dispatch"
]
batch_counts = [event.get("meta", {}).get("count") for event in batch_done]
assert batch_counts == [4, 2], batch_counts
PY

echo "[code-agent] user-facing explore command offline run"
cargo run -q -p air-cli -- code "explore command_run safety" \
  --target crates/air-tools/src/lib.rs \
  --query command_run \
  > target/generated/code_command_explore.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_explore.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
exploration = output["exploration"]
assert exploration["summary"].startswith("Fixture exploration completed")
assert any(item["path"] == "crates/air-tools/src/lib.rs" for item in exploration["relevant_files"])
assert exploration["next_steps"], exploration
PY

echo "[code-agent] persistent session carries turn context"
rm -f target/generated/code_agent_session.json
cargo run -q -p air-cli -- code "explore command_run safety" \
  --target crates/air-tools/src/lib.rs \
  --query command_run \
  --session target/generated/code_agent_session.json \
  --trace-out target/generated/code_agent_session_turn1.trace.jsonl \
  > target/generated/code_agent_session_turn1.output.json
cargo run -q -p air-cli -- code "continue from the previous AIR code-agent turn" \
  --target crates/air-tools/src/lib.rs \
  --query command_run \
  --session target/generated/code_agent_session.json \
  --trace-out target/generated/code_agent_session_turn2.trace.jsonl \
  > target/generated/code_agent_session_turn2.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_session.json", encoding="utf-8") as handle:
    session = json.load(handle)
assert session["version"] == 1, session
assert len(session["turns"]) == 2, session
assert session["turns"][0]["recipe"] == "explore", session
assert session["turns"][1]["completed"] is True, session

with open("target/generated/code_agent_session_turn2.trace.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]
model_calls = [
    event for event in trace
    if event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_explorer"
]
assert model_calls, trace
task = model_calls[0]["input"]["task"]
assert "AIR session context from previous turns" in task, task
assert "Fixture exploration completed" in task, task
PY

echo "[code-agent] repair fixture starts failing with a structured diagnostic"
if node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_repair_fixture.log 2>&1; then
  echo "repair fixture unexpectedly passed; it should start from a failing implementation" >&2
  cat target/generated/code_agent_repair_fixture.log >&2
  exit 1
fi
grep -q "examples/code-agent/repair-fixture/math.js:2:10: error:" \
  target/generated/code_agent_repair_fixture.log

echo "[code-agent] multifile repair fixture starts failing with structured diagnostics"
if node examples/code-agent/repair-multifile/test.js > target/generated/code_agent_repair_multifile_fixture.log 2>&1; then
  echo "multifile repair fixture unexpectedly passed; it should start from a failing implementation" >&2
  cat target/generated/code_agent_repair_multifile_fixture.log >&2
  exit 1
fi
grep -q "examples/code-agent/repair-multifile/math.js:4:10: error:" \
  target/generated/code_agent_repair_multifile_fixture.log

echo "[code-agent] core explore-repair offline run"
repair_fixture_backup="$(mktemp)"
repair_fixture_user_dirty_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$repair_fixture_backup"
cp examples/code-agent/repair-fixture/test.js "$repair_fixture_user_dirty_backup"
restore_core_repair_fixture() {
  cp "$repair_fixture_backup" examples/code-agent/repair-fixture/math.js
  cp "$repair_fixture_user_dirty_backup" examples/code-agent/repair-fixture/test.js
  rm -f "$repair_fixture_backup" "$repair_fixture_user_dirty_backup"
}
trap restore_core_repair_fixture EXIT
printf "\n// pre-existing user dirty change that repair diff must not include\n" >> examples/code-agent/repair-fixture/test.js
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/repair-core.air-profile.yaml \
  --trace-out target/generated/code_agent_repair_core.trace.jsonl \
  > target/generated/code_agent_repair_core.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_repair_core.post_test.log
restore_core_repair_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_repair_core.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_repair_core.trace.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

assert output["exploration"]["relevant_files"], output
assert output["repair_context"]["related_files"], output
repair = output["repair"]
assert repair["initial_success"] is False, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert repair["changed_files"], repair
assert repair["workspace_changed_files"], repair
assert repair["workspace_clean_before"] is False, repair
preexisting = {entry["path"] for entry in repair["preexisting_changed_files"]}
assert "examples/code-agent/repair-fixture/test.js" in preexisting, repair
assert "examples/code-agent/repair-fixture/math.js" in repair["workspace_diff"]["diff"], repair
assert "examples/code-agent/repair-fixture/test.js" not in repair["workspace_diff"]["diff"], repair
assert any(
    event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_explorer"
    and event.get("status") == "ok"
    for event in trace
), trace
assert any(
    event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_repair_context_selector"
    and event.get("status") == "ok"
    for event in trace
), trace
assert any(
    event.get("action") == "approval"
    and event.get("status") == "ok"
    and "file.write" in event.get("meta", {}).get("approval_for", [])
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.patch"
    and event.get("status") == "ok"
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "todo.write"
    and event.get("status") == "ok"
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "todo.read"
    and event.get("status") == "ok"
    for event in trace
), trace
repair_calls = [
    event for event in trace
    if event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_repairer"
]
assert repair_calls, trace
assert repair_calls[0]["input"]["progress"]["in_progress_count"] == 1, repair_calls[0]["input"]
assert any(
    todo["id"] == "repair" and todo["status"] == "in_progress"
    for todo in repair_calls[0]["input"]["progress"]["todos"]
), repair_calls[0]["input"]
PY

echo "[code-agent] user-facing code command offline run"
code_command_fixture_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$code_command_fixture_backup"
restore_code_command_fixture() {
  cp "$code_command_fixture_backup" examples/code-agent/repair-fixture/math.js
  rm -f "$code_command_fixture_backup"
}
trap restore_code_command_fixture EXIT
cargo run -q -p air-cli -- code "fix the failing add function and retest" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --related examples/code-agent/repair-fixture/test.js \
  --trace-out target/generated/code_agent_code_command.trace.jsonl \
  > target/generated/code_agent_code_command.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_code_command.post_test.log
restore_code_command_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_code_command.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_code_command.trace.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

assert output["exploration"]["relevant_files"], output
assert output["repair_context"]["related_files"], output
repair = output["repair"]
changed = {entry["path"] for entry in repair["changed_files"]}
assert repair["initial_success"] is False, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert "examples/code-agent/repair-fixture/math.js" in changed, repair
assert "examples/code-agent/repair-fixture/math.js" in repair["workspace_diff"]["diff"], repair
assert any(
    event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_repair_context_selector"
    and event.get("status") == "ok"
    for event in trace
), trace
assert any(
    event.get("action") == "approval"
    and event.get("status") == "ok"
    and "file.write" in event.get("meta", {}).get("approval_for", [])
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.patch"
    and event.get("status") == "ok"
    for event in trace
), trace
PY

echo "[code-agent] bounded loop repair command offline run"
code_loop_fixture_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$code_loop_fixture_backup"
restore_code_loop_fixture() {
  cp "$code_loop_fixture_backup" examples/code-agent/repair-fixture/math.js
  rm -f "$code_loop_fixture_backup"
}
trap restore_code_loop_fixture EXIT
cargo run -q -p air-cli -- code "fix the failing add function until tests pass" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --related examples/code-agent/repair-fixture/test.js \
  --loop \
  --max-iterations 2 \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_code_loop.trace.jsonl \
  > target/generated/code_agent_code_loop.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_code_loop.post_test.log
restore_code_loop_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_agent_code_loop.output.json", encoding="utf-8") as handle:
    output = json.load(handle)

assert output["status"] == "completed", output
assert output["completed"] is True, output
assert output["recipe"] == "repair", output
assert len(output["iterations"]) == 1, output
repair = output["final_outputs"]["repair"]
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert "examples/code-agent/repair-fixture/math.js" in repair["workspace_diff"]["diff"], repair
assert Path("target/generated/code_agent_code_loop.trace.iter1.jsonl").exists()
PY

echo "[code-agent] bounded loop carries failed iteration context"
code_loop_feedback_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$code_loop_feedback_backup"
restore_code_loop_feedback_fixture() {
  cp "$code_loop_feedback_backup" examples/code-agent/repair-fixture/math.js
  rm -f "$code_loop_feedback_backup"
}
trap restore_code_loop_feedback_fixture EXIT
cargo run -q -p air-cli -- code "fix the failing add function until tests pass" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --related examples/code-agent/repair-fixture/test.js \
  --loop \
  --max-iterations 2 \
  --model-config examples/code-agent/model-fixtures.loop-fail.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_code_loop_feedback.trace.jsonl \
  > target/generated/code_agent_code_loop_feedback.output.json
restore_code_loop_feedback_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_agent_code_loop_feedback.output.json", encoding="utf-8") as handle:
    output = json.load(handle)

assert output["status"] == "max_iterations_exhausted", output
assert output["completed"] is False, output
assert len(output["iterations"]) == 2, output
assert output["iterations"][0]["outputs"]["repair"]["final_success"] is False, output
assert Path("target/generated/code_agent_code_loop_feedback.trace.iter2.jsonl").exists()

with open("target/generated/code_agent_code_loop_feedback.trace.iter2.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

repair_calls = [
    event for event in trace
    if event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_repairer"
]
assert repair_calls, trace
task = repair_calls[0]["input"]["task"]
assert "AIR loop context from previous iterations" in task, task
assert '"final_success":false' in task, task
assert "Patch validation failed before apply" in task, task
PY

echo "[code-agent] core multifile repair offline run"
multifile_math_backup="$(mktemp)"
multifile_normalize_backup="$(mktemp)"
cp examples/code-agent/repair-multifile/math.js "$multifile_math_backup"
cp examples/code-agent/repair-multifile/normalize.js "$multifile_normalize_backup"
restore_multifile_repair_fixture() {
  cp "$multifile_math_backup" examples/code-agent/repair-multifile/math.js
  cp "$multifile_normalize_backup" examples/code-agent/repair-multifile/normalize.js
  rm -f "$multifile_math_backup" "$multifile_normalize_backup"
}
trap restore_multifile_repair_fixture EXIT
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/repair-multifile.air-profile.yaml \
  --trace-out target/generated/code_agent_repair_multifile.trace.jsonl \
  > target/generated/code_agent_repair_multifile.output.json
node examples/code-agent/repair-multifile/test.js > target/generated/code_agent_repair_multifile.post_test.log
restore_multifile_repair_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_repair_multifile.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_repair_multifile.trace.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

repair = output["repair"]
changed = {entry["path"] for entry in repair["changed_files"]}
workspace_changed = {entry["path"] for entry in repair["workspace_changed_files"]}
assert output["repair_context"]["related_files"], output
assert repair["initial_success"] is False, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert "examples/code-agent/repair-multifile/math.js" in changed, repair
assert "examples/code-agent/repair-multifile/normalize.js" in changed, repair
assert changed <= workspace_changed, repair
assert "examples/code-agent/repair-multifile/math.js" in repair["workspace_diff"]["diff"], repair
assert "examples/code-agent/repair-multifile/normalize.js" in repair["workspace_diff"]["diff"], repair
assert any(
    event.get("action") == "approval"
    and event.get("status") == "ok"
    and "file.write" in event.get("meta", {}).get("approval_for", [])
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.patch"
    and event.get("status") == "ok"
    and event.get("output", {}).get("file_count") == 2
    for event in trace
), trace
PY

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
