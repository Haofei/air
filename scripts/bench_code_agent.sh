#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p target/generated/code-agent-bench

run_route() {
  local name="$1"
  local task="$2"
  local expected_id="$3"
  local expected_source="$4"
  local output="target/generated/code-agent-bench/route_${name}.json"

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
PY
}

echo "[code-agent-bench] route primary task shapes"
run_route \
  "review" \
  "Review the command_run implementation for safety, provenance, and diagnostics." \
  "code.review_with_std_context@0.1.0" \
  "recipe"
run_route \
  "edit" \
  "Edit a failing test using OpenCode-style read/edit/verify steps, and retest." \
  "code.edit_loop@0.1.0" \
  "recipe"
run_route \
  "explore" \
  "Explore how command_run is implemented and identify relevant repository files." \
  "code.explore@0.1.0" \
  "module"

echo "[code-agent-bench] offline edit loop"
fixture="examples/code-agent/edit-fixture/math.js"
backup="$(mktemp)"
cp "$fixture" "$backup"
restore_fixture() {
  cp "$backup" "$fixture"
  rm -f "$backup"
}
trap restore_fixture EXIT

cargo run -q -p air-cli -- run-plan --profile examples/code-agent/edit.air-profile.yaml \
  --trace-out target/generated/code-agent-bench/edit.trace.jsonl \
  > target/generated/code-agent-bench/edit.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code-agent-bench/edit.post_test.log
restore_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code-agent-bench/edit.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code-agent-bench/edit.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
assert "edit" in tools, tools
assert "read" in tools, tools
assert "test.run" in tools, tools
assert "git.diff" in tools, tools
assert any(event.get("action") == "tool_batch_dispatch" for event in events), events
PY

echo "[code-agent-bench] fuzzy edit fallback"
backup="$(mktemp)"
cp "$fixture" "$backup"
trap restore_fixture EXIT

cargo run -q -p air-cli -- run-plan --profile examples/code-agent/edit.air-profile.yaml \
  --model-config examples/code-agent/fixtures/model-fixtures.fuzzy-line-trimmed.json \
  --trace-out target/generated/code-agent-bench/fuzzy-line-trimmed.trace.jsonl \
  > target/generated/code-agent-bench/fuzzy-line-trimmed.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code-agent-bench/fuzzy-line-trimmed.post_test.log
restore_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code-agent-bench/fuzzy-line-trimmed.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit

with open("target/generated/code-agent-bench/fuzzy-line-trimmed.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
edit_events = [
    event
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") == "edit"
]
assert len(edit_events) == 1, edit_events
assert set(["filePath", "oldString", "newString"]).issubset(edit_events[0]["input"]), edit_events[0]
assert "match_strategy" not in edit_events[0]["input"], edit_events[0]
assert edit_events[0]["output"]["match_strategies"] == ["line_trimmed"], edit_events[0]
assert edit_events[0]["output"]["applied"] is True, edit_events[0]
PY

echo "[code-agent-bench] ok"
