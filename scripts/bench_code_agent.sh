#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p target/generated/code-agent-bench

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
assert "bash" in tools, tools
assert any(event.get("action") == "tool_batch_dispatch" for event in events), events
PY

echo "[code-agent-bench] ok"
