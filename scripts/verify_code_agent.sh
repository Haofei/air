#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p target/generated

echo "[code-agent] validate minimal profile"
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/edit.air-profile.yaml

echo "[code-agent] static minimal-loop checks"
"${PYTHON:-python3}" - <<'PY'
import json
import re
from pathlib import Path

module = Path("examples/code-agent/code-edit-loop.air.yaml").read_text()
rule_ids = re.findall(r"^\s+- id:\s*([A-Za-z0-9_-]+)\s*$", module, re.M)
assert rule_ids == [
    "init",
    "summarize-at-step-limit",
    "choose",
    "act",
    "edit-applied",
    "bash-passed-after-patch",
    "bash-passed-before-patch",
    "bash-failed",
    "complete-needs-verification",
    "complete-verified",
    "continue-after-act",
    "summarize",
    "done",
], rule_ids

expected_tools = ["question", "bash", "read", "glob", "grep", "edit", "write", "task", "webfetch", "todowrite", "todoread", "skill"]
for required in expected_tools:
    assert re.search(rf"\bname:\s*{re.escape(required)}\b", module) or f"{required}:" in module, required

choose = re.search(r"- id:\s*choose\s*\n(?P<body>.*?)(?:\n\s*-\s+id:|\Z)", module, re.S)
assert choose and "model: code_edit_decider" in choose.group("body")
assert "max_items: 12" in choose.group("body")
assert "max_bytes: 180000" in choose.group("body")

for path in (
    "examples/code-agent/tools.json",
    "examples/code-agent/tools.dogfood.json",
    "examples/code-agent/fixtures/tools.core.json",
    "examples/code-agent/fixtures/tools.playwright.json",
):
    tools = json.loads(Path(path).read_text())["tools"]
    assert set(tools) == set(expected_tools), (path, list(tools))
PY

echo "[code-agent] fixture smoke"
fixture="examples/code-agent/edit-fixture/math.js"
backup="$(mktemp)"
cp "$fixture" "$backup"
cleanup() {
  cp "$backup" "$fixture"
  rm -f "$backup"
}
trap cleanup EXIT

cargo run -q -p air-cli -- run-plan \
  --profile examples/code-agent/edit.air-profile.yaml \
  --trace-out target/generated/code-agent-minimal.trace.jsonl \
  >/tmp/air-code-agent-minimal.out

"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

output = json.loads(Path("/tmp/air-code-agent-minimal.out").read_text())
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit
assert "examples/code-agent/edit-fixture/math.js" in edit["workspace_diff"]["diff"], edit

trace = [json.loads(line) for line in Path("target/generated/code-agent-minimal.trace.jsonl").read_text().splitlines()]
first_decider = next(event for event in trace if event.get("action") == "model_call_start" and event.get("meta", {}).get("model") == "code_edit_decider")
tool_names = first_decider["input"]["allowed_tools"]
assert tool_names == ["question", "bash", "read", "glob", "grep", "edit", "write", "task", "webfetch", "todowrite", "todoread", "skill"], tool_names
PY

echo "[code-agent] ok"
