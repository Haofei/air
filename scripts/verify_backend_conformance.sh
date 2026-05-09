#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MODEL_CONFIG="${AIR_CONFORMANCE_MODEL_CONFIG:-examples/bigmodel-openai-compatible.json}"
LANGGRAPH_PYTHON="${AIR_LANGGRAPH_PYTHON:-.venv/bin/python}"
OUT="target/generated/conformance"

mkdir -p "$OUT"

echo "[air-conformance] approval gate matrix"
cargo run -q -p air-cli -- validate-plan tests/plans/approval-smoke.air-plan.yaml \
  --store tests/plans/approval-smoke.air-store.yaml
cargo run -q -p air-cli -- run-plan tests/plans/approval-smoke.air-plan.yaml \
  --store tests/plans/approval-smoke.air-store.yaml \
  --input tests/plans/approval-smoke.input.json \
  --tool-config tests/plans/approval-smoke.tools.json \
  --trace-out "$OUT/approval.native.trace.jsonl" \
  > "$OUT/approval.native.output.json"
cargo run -q -p air-cli -- lower-plan tests/plans/approval-smoke.air-plan.yaml \
  --store tests/plans/approval-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output "$OUT/approval.openai.mjs"
AIR_TRACE=1 node "$OUT/approval.openai.mjs" \
  --input tests/plans/approval-smoke.input.json \
  --model-config "$MODEL_CONFIG" \
  --tool-config tests/plans/approval-smoke.tools.json \
  > "$OUT/approval.openai.output.json" \
  2> "$OUT/approval.openai.trace.jsonl"
cargo run -q -p air-cli -- lower-plan tests/plans/approval-smoke.air-plan.yaml \
  --store tests/plans/approval-smoke.air-store.yaml \
  --backend langgraph \
  --output "$OUT/approval.langgraph.py"
"$LANGGRAPH_PYTHON" - <<'PY' > target/generated/conformance/approval.langgraph.output.json
import importlib.util
import json

spec = importlib.util.spec_from_file_location("approval_conformance", "target/generated/conformance/approval.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.AIR_APPROVAL_PROVIDER = lambda **kwargs: {
    "approved": True,
    "approver": "conformance",
    "reason": "approval conformance test",
}
print(json.dumps(module.graph.invoke({"deployment_id": "deploy-123"}), indent=2))
PY
"$LANGGRAPH_PYTHON" - <<'PY'
import json

def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)

native = load("target/generated/conformance/approval.native.output.json")["result"]
openai = load("target/generated/conformance/approval.openai.output.json")["result"]
langgraph = load("target/generated/conformance/approval.langgraph.output.json")["result"]
assert native == openai == langgraph == {"approved": True, "deployment_id": "deploy-123"}

for path in [
    "target/generated/conformance/approval.native.trace.jsonl",
    "target/generated/conformance/approval.openai.trace.jsonl",
]:
    with open(path, encoding="utf-8") as handle:
        events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
    assert any(event["action"] == "approval" and event["status"] == "ok" for event in events)
PY

echo "[air-conformance] tool capability matrix"
cargo run -q -p air-cli -- validate tests/agents/tool-capability-smoke.air.yaml
cargo run -q -p air-cli -- validate-plan tests/plans/tool-capability-smoke.air-plan.yaml \
  --store tests/plans/tool-capability-smoke.air-store.yaml
cargo run -q -p air-cli -- run-plan tests/plans/tool-capability-smoke.air-plan.yaml \
  --store tests/plans/tool-capability-smoke.air-store.yaml \
  --input tests/plans/tool-capability-smoke.input.json \
  --tool-config tests/plans/tool-capability-smoke.tools.json \
  --trace-out "$OUT/tool.native.trace.jsonl" \
  > "$OUT/tool.native.output.json"
cargo run -q -p air-cli -- lower-plan tests/plans/tool-capability-smoke.air-plan.yaml \
  --store tests/plans/tool-capability-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output "$OUT/tool.openai.mjs"
AIR_TRACE=1 node "$OUT/tool.openai.mjs" \
  --input tests/plans/tool-capability-smoke.input.json \
  --model-config "$MODEL_CONFIG" \
  --tool-config tests/plans/tool-capability-smoke.tools.json \
  > "$OUT/tool.openai.output.json" \
  2> "$OUT/tool.openai.trace.jsonl"
cargo run -q -p air-cli -- lower-plan tests/plans/tool-capability-smoke.air-plan.yaml \
  --store tests/plans/tool-capability-smoke.air-store.yaml \
  --backend langgraph \
  --output "$OUT/tool.langgraph.py"
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY' > target/generated/conformance/tool.langgraph.output.json
import importlib.util
import json

spec = importlib.util.spec_from_file_location("tool_conformance", "target/generated/conformance/tool.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.AIR_TOOL_CAPABILITIES = {"docs.search": "retrieval.local"}
module.AIR_TOOL_PROVIDER = lambda *, name, input_value: {
    "query": input_value["query"],
    "documents": [{"id": "refund", "title": "Refund policy", "content": "Refund requests route to billing."}],
}
print(json.dumps(module.graph.invoke({"text": "refund policy"})["result"], indent=2))
PY
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY'
import importlib.util
import json

def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)

assert load("target/generated/conformance/tool.native.output.json")["result"]["query"] == "refund policy"
assert load("target/generated/conformance/tool.openai.output.json")["result"]["query"] == "refund policy"
assert load("target/generated/conformance/tool.langgraph.output.json")["query"] == "refund policy"

with open("target/generated/conformance/tool.native.trace.jsonl", encoding="utf-8") as handle:
    native_events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "tool_call" and event["status"] == "ok" for event in native_events)

with open("target/generated/conformance/tool.openai.trace.jsonl", encoding="utf-8") as handle:
    openai_events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "tool_call" and event["status"] == "ok" for event in openai_events)

spec = importlib.util.spec_from_file_location("tool_conformance_bad", "target/generated/conformance/tool.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.AIR_TOOL_CAPABILITIES = {"docs.search": "network.search"}
module.AIR_TOOL_PROVIDER = lambda *, name, input_value: {"query": input_value["query"], "documents": []}
try:
    module.graph.invoke({"text": "refund policy"})
except RuntimeError as error:
    assert "provider capability network.search" in str(error)
else:
    raise AssertionError("expected provider capability mismatch")
PY

echo "[air-conformance] dynamic fan-out matrix"
cargo run -q -p air-cli -- validate-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml
cargo run -q -p air-cli -- run-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input tests/fixtures/empty.input.json \
  --trace-out "$OUT/dynamic.native.trace.jsonl" \
  > "$OUT/dynamic.native.output.json"
cargo run -q -p air-cli -- lower-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output "$OUT/dynamic.openai.mjs"
AIR_TRACE=1 node "$OUT/dynamic.openai.mjs" \
  --input tests/fixtures/empty.input.json \
  --model-config "$MODEL_CONFIG" \
  > "$OUT/dynamic.openai.output.json" \
  2> "$OUT/dynamic.openai.trace.jsonl"
cargo run -q -p air-cli -- lower-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend langgraph \
  --output "$OUT/dynamic.langgraph.py"
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY' > target/generated/conformance/dynamic.langgraph.output.json
import importlib.util
import json

spec = importlib.util.spec_from_file_location("dynamic_conformance", "target/generated/conformance/dynamic.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
print(json.dumps(module.graph.invoke({}), indent=2))
PY
"$LANGGRAPH_PYTHON" - <<'PY'
import json

def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)

assert load("target/generated/conformance/dynamic.native.output.json")["report"] == "done"
assert load("target/generated/conformance/dynamic.openai.output.json")["report"] == "done"
assert load("target/generated/conformance/dynamic.langgraph.output.json")["report"] == "done"

with open("target/generated/conformance/dynamic.native.trace.jsonl", encoding="utf-8") as handle:
    native_events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(
    event["agent"] == "$planner"
    and event["rule"] == "dynamic_fanout"
    and event["action"] == "materialize"
    and event["output"]["nodes"] == ["topic_1", "topic_2"]
    for event in native_events
)
assert any(
    event["action"] == "return" and event["output"] == {"report": "done"}
    for event in native_events
)

with open("target/generated/conformance/dynamic.openai.trace.jsonl", encoding="utf-8") as handle:
    openai_events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "dynamic_fanout" and event["meta"]["count"] == 2 for event in openai_events)
assert any(event["agent"] == "final" and event["action"] == "return" and event["output"] == "done" for event in openai_events)
PY

echo "[air-conformance] backend conformance complete"
