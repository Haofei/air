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

echo "[air-conformance] tool batch dispatch matrix"
cargo run -q -p air-cli -- validate-plan tests/plans/tool-batch-smoke.air-plan.yaml \
  --store tests/plans/tool-batch-smoke.air-store.yaml
cargo run -q -p air-cli -- run-plan tests/plans/tool-batch-smoke.air-plan.yaml \
  --store tests/plans/tool-batch-smoke.air-store.yaml \
  --input tests/plans/tool-batch-smoke.input.json \
  --tool-config tests/plans/tool-batch-smoke.tools.json \
  --trace-out "$OUT/tool_batch.native.trace.jsonl" \
  > "$OUT/tool_batch.native.output.json"
cargo run -q -p air-cli -- lower-plan tests/plans/tool-batch-smoke.air-plan.yaml \
  --store tests/plans/tool-batch-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output "$OUT/tool_batch.openai.mjs"
AIR_TRACE=1 node "$OUT/tool_batch.openai.mjs" \
  --input tests/plans/tool-batch-smoke.input.json \
  --model-config "$MODEL_CONFIG" \
  --tool-config tests/plans/tool-batch-smoke.tools.json \
  > "$OUT/tool_batch.openai.output.json" \
  2> "$OUT/tool_batch.openai.trace.jsonl"
cargo run -q -p air-cli -- lower-plan tests/plans/tool-batch-smoke.air-plan.yaml \
  --store tests/plans/tool-batch-smoke.air-store.yaml \
  --backend langgraph \
  --output "$OUT/tool_batch.langgraph.py"
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY' > target/generated/conformance/tool_batch.langgraph.output.json
import importlib.util
import json

spec = importlib.util.spec_from_file_location("tool_batch_conformance", "target/generated/conformance/tool_batch.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.AIR_TOOL_CAPABILITIES = {"docs.search": "retrieval.local"}
module.AIR_TOOL_PROVIDER = lambda *, name, input_value: {
    "query": input_value["query"],
    "documents": [{"id": input_value["query"], "title": input_value["query"].title(), "content": "fixture"}],
}
inputs = json.load(open("tests/plans/tool-batch-smoke.input.json", encoding="utf-8"))
print(json.dumps(module.graph.invoke(inputs), indent=2))
PY
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY'
import json

def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)

native = load("target/generated/conformance/tool_batch.native.output.json")["observations"]
openai = load("target/generated/conformance/tool_batch.openai.output.json")["observations"]
langgraph = load("target/generated/conformance/tool_batch.langgraph.output.json")["observations"]
for observations in [native, openai, langgraph]:
    assert len(observations) == 2, observations
    assert [item["tool"] for item in observations] == ["docs.search", "docs.search"], observations
    assert [item["input"]["query"] for item in observations] == ["alpha", "beta"], observations

with open("target/generated/conformance/tool_batch.native.trace.jsonl", encoding="utf-8") as handle:
    native_events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "tool_batch_dispatch" and event["status"] == "ok" for event in native_events)

with open("target/generated/conformance/tool_batch.openai.trace.jsonl", encoding="utf-8") as handle:
    openai_events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "tool_batch_dispatch" and event["status"] == "ok" for event in openai_events)
assert any(event["action"] == "tool_batch_dispatch_item" and event["meta"]["tool"] == "docs.search" for event in openai_events)
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
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY'
import importlib.util
import threading
import time

spec = importlib.util.spec_from_file_location("dynamic_parallel_conformance", "target/generated/conformance/dynamic.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
original_run_module = module._air_run_module
lock = threading.Lock()
active = {"count": 0, "max": 0}

def run_module(module_id, inputs):
    if module_id != "test.topic_note@0.1.0":
        return original_run_module(module_id, inputs)
    with lock:
        active["count"] += 1
        active["max"] = max(active["max"], active["count"])
    try:
        time.sleep(0.05)
        return original_run_module(module_id, inputs)
    finally:
        with lock:
            active["count"] -= 1

module._air_run_module = run_module
assert module.graph.invoke({})["report"] == "done"
assert active["max"] >= 2, active
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

echo "[air-conformance] langgraph run-plan condition and halt semantics"
cargo run -q -p air-cli -- lower-plan examples/deep-research/deep-research-supervised-two-step.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --backend langgraph \
  --output "$OUT/deep_supervised.langgraph.py"
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY'
import importlib.util

spec = importlib.util.spec_from_file_location("deep_supervised_conformance", "target/generated/conformance/deep_supervised.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

def model(name, input_value):
    if name == "research_planner_array":
        return {"research_brief": "brief", "topics": ["t1", "t2", "t3", "t4"]}
    if name == "research_refiner":
        return {"complete": True, "follow_up_query": "", "rationale": "enough"}
    if name == "research_compressor":
        return {"topic": input_value["topic"], "summary": f"summary {input_value['topic']}", "sources": [f"src-{input_value['topic']}"]}
    if name == "research_supervisor":
        return {"action": "research_complete", "needs_more": False, "follow_up_topics": ["skip-a", "skip-b"], "rationale": "done"}
    if name == "final_reporter":
        return {"report": f"notes={len(input_value['notes'])}", "sources": [], "limitations": []}
    raise AssertionError(name)

def tool(name, input_value):
    if name == "web.search":
        return {"query": input_value["query"], "documents": [{"id": "doc", "title": "Doc", "content": "Content"}]}
    if name == "research.think":
        return {"reflection": "ok"}
    raise AssertionError(name)

module.call_model = model
module.AIR_TOOL_PROVIDER = lambda *, name, input_value: tool(name, input_value)
module.AIR_TOOL_CAPABILITIES = {"web.search": "network.search", "research.think": "research.reflect"}
result = module.graph.invoke({"question": "q"})
assert result["result"]["report"] == "notes=2"
for node in ["topic_3", "topic_4", "supervisor_2", "topic_5", "topic_6"]:
    assert result[node]["__air_skipped"] is True, node
PY

cargo run -q -p air-cli -- lower-plan examples/deep-research/deep-research-clarified.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --backend langgraph \
  --output "$OUT/deep_clarified.langgraph.py"
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY'
import importlib.util

spec = importlib.util.spec_from_file_location("deep_clarified_conformance", "target/generated/conformance/deep_clarified.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

def model(name, input_value):
    assert name == "research_clarifier", name
    return {
        "needs_clarification": True,
        "question": input_value["question"],
        "verification": "Need scope",
        "normalized_question": input_value["question"],
        "assumptions": [],
    }

module.call_model = model
result = module.graph.invoke({"question": "research AI"})
assert result["clarification"]["needs_clarification"] is True
assert result["__air_halted"]["after"] == "clarify"
assert "result" not in result
PY

cargo run -q -p air-cli -- lower-plan examples/deep-research/deep-research-dynamic.air-plan.yaml \
  --store examples/deep-research/module-store.air-store.yaml \
  --backend langgraph \
  --output "$OUT/deep_dynamic_capability.langgraph.py"
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY'
import importlib.util
from pathlib import Path

source = Path("target/generated/conformance/deep_dynamic_capability.langgraph.py")
target = Path("target/generated/conformance/deep_dynamic_missing_capability.langgraph.py")
text = source.read_text(encoding="utf-8")
needle = '"requires": {"capabilities": ["network.search", "research.reflect"]}'
assert needle in text
target.write_text(text.replace(needle, '"requires": {"capabilities": []}', 1), encoding="utf-8")

spec = importlib.util.spec_from_file_location("deep_dynamic_missing_capability", target)
module = importlib.util.module_from_spec(spec)
try:
    spec.loader.exec_module(module)
except RuntimeError as error:
    assert "RunPlan uses dynamic fanout" in str(error)
    assert "network.search" in str(error) or "research.reflect" in str(error)
else:
    raise AssertionError("expected generated LangGraph runtime to reject missing dynamic fanout capability")
PY

echo "[air-conformance] backend conformance complete"
