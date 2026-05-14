#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p target/generated

echo "[code-agent] validate primitive profiles"
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/edit.air-profile.yaml

echo "[code-agent] OpenCode-style edit kernel checks"
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
    "manual-format-failed",
    "manual-format-test-failed",
    "manual-format-test-passed-with-assertions",
    "manual-format-test-passed",
    "manual-format-passed",
    "final-format-before-complete",
    "final-format-tool-error",
    "final-format-failed",
    "final-verify-after-format",
    "final-verify-truncated-failed",
    "final-verify-failed",
    "final-verify-passed-with-assertions",
    "final-acceptance-assertions-failed",
    "final-acceptance-assertions-passed",
    "acceptance-assertions-failed",
    "acceptance-assertions-passed",
    "final-verify-passed",
    "manual-test-failed",
    "manual-test-passed-with-assertions",
    "manual-test-passed",
    "edit-validation-failed",
    "continue-after-act",
    "summarize",
    "done",
], rule_ids

choose = re.search(r"- id:\s*choose\s*\n(?P<body>.*?)(?:\n\s*-\s+id:|\Z)", module, re.S)
assert choose, "missing choose rule"
assert "model: code_edit_decider" in choose.group("body"), choose.group("body")
assert "literal: format.run" in re.search(
    r"allowed_tools:\s*\n(?P<body>.*?)tool_schemas:",
    choose.group("body"),
    re.S,
).group("body"), "formatter should be model-selected like OpenCode, with AIR still enforcing the final gate"

observations_window = re.search(
    r"observations:\s*\n\s+take_last_within_bytes:\s*\n\s+ref:\s*observations\s*\n\s+max_items:\s*(\d+)\s*\n\s+max_bytes:\s*(\d+)",
    choose.group("body"),
)
assert observations_window, "choose rule must pass bounded recent observations"
assert int(observations_window.group(1)) <= 4, observations_window.group(0)
assert int(observations_window.group(2)) <= 24000, observations_window.group(0)

for phrase in (
    "Work as a direct coding tool loop",
    "Do not repeat the same read/search",
    "When the task names an exact file path",
    "do not validate after every tiny edit",
):
    assert phrase in choose.group("body"), phrase

decision_schema = re.search(
    r"\n\s+decision:\s*\n(?P<body>.*?)(?:\n\s+observation:)",
    module,
    re.S,
)
assert decision_schema, "missing decision state schema"
assert "required: [complete, tool_calls]" in decision_schema.group("body"), decision_schema.group("body")
assert "rationale:" not in decision_schema.group("body"), "code edit decider should not require AIR rationale wrapper"

act_body = re.search(
    r"- id:\s*act\s*\n(?P<body>.*?)(?:\n\s*-\s+id:|\Z)",
    module,
    re.S,
).group("body")
assert "path: decision.rationale" not in act_body, "act observations should not replay model rationale"

edit_contract = re.search(
    r"\n\s+edit:\s*\n(?P<body>.*?)(?:\n\s+diagnostic\.context:)",
    choose.group("body"),
    re.S,
)
assert edit_contract, "choose rule must expose the edit tool contract"
edit_contract_body = edit_contract.group("body")
for key in ("filePath", "oldString", "newString", "replaceAll", "edits"):
    assert key in edit_contract_body, (key, edit_contract_body)
assert "literal: format.run" in module, "formatter tool must be declared and used"
assert "literal: test.run" in module, "validation tool must be declared and used"
assert "literal: git.diff" in module, "summary must capture final diff"
assert "Copy repository paths exactly byte-for-byte from tool output metadata" in module, "path policy should not depend on user-supplied hints"

summarize = re.search(r"- id:\s*summarize\s*\n(?P<body>.*?)(?:\n\s*-\s+id:|\Z)", module, re.S)
assert summarize, "missing summarize rule"
assert "model: code_edit_summarizer" in summarize.group("body"), summarize.group("body")
summary_observations_window = re.search(
    r"observations:\s*\n\s+take_last_within_bytes:\s*\n\s+ref:\s*observations\s*\n\s+max_items:\s*(\d+)\s*\n\s+max_bytes:\s*(\d+)",
    summarize.group("body"),
)
assert summary_observations_window, "summarizer must use bounded final observations"
assert int(summary_observations_window.group(1)) <= 8, summary_observations_window.group(0)
assert int(summary_observations_window.group(2)) <= 30000, summary_observations_window.group(0)

config = json.loads(Path("examples/bigmodel-openai-compatible.json").read_text())
decider = config["models"]["code_edit_decider"]
assert decider.get("native_tool_calls") is True, decider
assert decider.get("request_timeout_seconds", 0) >= 180, decider
prompt = decider.get("system_prompt", "").lower()
for phrase in (
    "opencode-style",
    "direct tool loop",
    "do not keep rereading the same ranges",
    "targeted refactors that need a checklist",
    "repository lint style",
    "too_many_arguments",
    "type_complexity",
):
    assert phrase in prompt, phrase

self_tools = json.loads(Path("examples/code-agent/tools.dogfood.json").read_text())["tools"]
assert self_tools["edit"].get("max_changed_lines", 0) >= 800, self_tools["edit"]
assert "format.run" in self_tools
for path in (
    "examples/code-agent/tools.json",
    "examples/code-agent/fixtures/tools.core.json",
    "examples/code-agent/tools.dogfood.json",
    "examples/code-agent/fixtures/tools.playwright.json",
):
    tools = json.loads(Path(path).read_text())["tools"]
    assert tools["grep"].get("kind") == "file_search", path
    assert tools["grep"].get("max_matches") <= 40, path
    assert "lsp.references" in tools and tools["lsp.references"]["kind"] == "rust_analyzer_references", path
    assert "lsp.diagnostics" in tools and tools["lsp.diagnostics"]["kind"] == "rust_analyzer_diagnostics", path
PY

echo "[code-agent] Rust regression tests"
cargo test -q -p air-cli code_agent_pack_declares_all_default_profiles
cargo test -q -p air-cli pack_validation_rejects
cargo test -q -p air-cli pack_input_contract
cargo test -q -p air-cli model_config_prompts_match_code_agent_schemas
cargo test -q -p air-cli edit_loop_
cargo test -q -p air-tools file_read
cargo test -q -p air-tools file_search
cargo test -q -p air-tools edit
cargo test -q -p air-tools git_status
cargo test -q -p air-tools command_run
cargo test -q -p air-tools diagnostic_context
cargo test -q -p air-tools rust_lsp_tools

echo "[code-agent] deterministic edit loop run"
backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$backup"
restore_fixture() {
  cp "$backup" examples/code-agent/edit-fixture/math.js
  rm -f "$backup"
}
trap restore_fixture EXIT
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/edit.air-profile.yaml \
  --trace-out target/generated/code_agent_edit.trace.jsonl \
  > target/generated/code_agent_edit.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit.post_test.log
restore_fixture
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
for tool in ("test.run", "read", "edit", "format.run", "git.diff"):
    assert tool in tools, tools
assert "candidate.validate" not in tools[:2], tools
assert any(
    event.get("rule") == "final-format-before-complete"
    and event.get("meta", {}).get("tool") == "format.run"
    for event in events
), events
assert any(
    event.get("rule") == "final-verify-after-format"
    and event.get("meta", {}).get("tool") == "test.run"
    for event in events
), events
assert any(
    event.get("rule") == "summarize"
    and event.get("meta", {}).get("tool") == "git.diff"
    for event in events
), events
models = [
    event.get("meta", {}).get("model")
    for event in events
    if event.get("action") == "model_call"
]
assert "code_edit_decider" in models, models
assert "code_edit_summarizer" in models, models
first_decider = next(
    event for event in events
    if event.get("action") == "model_call_start"
    and event.get("meta", {}).get("model") == "code_edit_decider"
)
assert "format.run" in first_decider["input"]["allowed_tools"], first_decider["input"]["allowed_tools"]
PY

echo "[code-agent] targetless edit loop run"
backup="$(mktemp)"
cp examples/code-agent/edit-fixture/math.js "$backup"
restore_fixture() {
  cp "$backup" examples/code-agent/edit-fixture/math.js
  rm -f "$backup"
}
trap restore_fixture EXIT
cargo run -q -p air-cli -- run-plan examples/code-agent/code-edit.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/fixtures/edit.targetless.input.json \
  --model-config examples/code-agent/fixtures/model-fixtures.targetless.json \
  --tool-config examples/code-agent/fixtures/tools.core.json \
  --trace-out target/generated/code_agent_edit_targetless.trace.jsonl \
  > target/generated/code_agent_edit_targetless.output.json
node examples/code-agent/edit-fixture/test.js > target/generated/code_agent_edit_targetless.post_test.log
restore_fixture
trap - EXIT

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_targetless.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
edit = output["edit"]
assert edit["final_success"] is True, edit
assert edit["patch_applied"] is True, edit
assert edit["changed_files"] == ["examples/code-agent/edit-fixture/math.js"], edit

with open("target/generated/code_agent_edit_targetless.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
tools = [
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("status") == "ok"
]
for tool in ("test.run", "glob", "grep", "read_many", "edit", "format.run", "git.diff"):
    assert tool in tools, tools
PY

echo "[code-agent] explain command"
cargo run -q -p air-cli -- code "edit the failing add function and retest" \
  --explain \
  > target/generated/code_agent_edit_explain.output.json

"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_edit_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
assert "resolved_recipe" not in output, output
assert "recipe" not in output["pack"], output
assert output["pack"]["default_profile"] == "examples/code-agent/edit.air-profile.yaml", output
assert "test_command" not in output["input"], output
PY

echo "[code-agent] ok"
