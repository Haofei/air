#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STORE="examples/deep-research/module-store.air-store.yaml"
INPUT="examples/deep-research/input.json"
TOOLS="examples/deep-research/tools.json"
MODEL_CONFIG="${AIR_DEEP_RESEARCH_MODEL_CONFIG:-examples/bigmodel-openai-compatible.json}"
LANGGRAPH_PYTHON="${AIR_LANGGRAPH_PYTHON:-.venv/bin/python}"

echo "[air-verify] formatting"
cargo fmt --check

echo "[air-verify] core tests"
cargo test

echo "[air-verify] deep research modules"
cargo run -p air-cli -- validate examples/deep-research/clarify-scope.air.yaml
cargo run -p air-cli -- validate examples/deep-research/research-plan.air.yaml
cargo run -p air-cli -- validate examples/deep-research/research-plan-array.air.yaml
cargo run -p air-cli -- validate examples/deep-research/research-topic.air.yaml
cargo run -p air-cli -- validate examples/deep-research/research-supervisor.air.yaml
cargo run -p air-cli -- validate examples/deep-research/final-report.air.yaml

echo "[air-verify] deep research plans"
cargo run -p air-cli -- validate-plan examples/deep-research/deep-research.air-plan.yaml --store "$STORE"
cargo run -p air-cli -- validate-plan examples/deep-research/deep-research-array.air-plan.yaml --store "$STORE"
cargo run -p air-cli -- validate-plan examples/deep-research/deep-research-clarified.air-plan.yaml --store "$STORE"
cargo run -p air-cli -- validate-plan examples/deep-research/deep-research-supervised.air-plan.yaml --store "$STORE"
cargo run -p air-cli -- validate-plan examples/deep-research/deep-research-supervised-two-step.air-plan.yaml --store "$STORE"
cargo run -p air-cli -- validate-plan examples/deep-research/deep-research-dynamic.air-plan.yaml --store "$STORE"

echo "[air-verify] deep research profile"
cargo run -p air-cli -- validate-plan --profile examples/deep-research/profile.air-profile.yaml

echo "[air-verify] parallel CLI smoke"
cargo run -p air-cli -- validate-plan tests/plans/parallel-smoke.air-plan.yaml --store tests/plans/parallel-smoke.air-store.yaml
cargo run -p air-cli -- run-plan tests/plans/parallel-smoke.air-plan.yaml \
  --store tests/plans/parallel-smoke.air-store.yaml \
  --input tests/fixtures/model-smoke.input.json \
  --parallel \
  --trace-out target/generated/parallel_smoke.trace.jsonl \
  > target/generated/parallel_smoke.output.json
cargo run -p air-cli -- replay target/generated/parallel_smoke.trace.jsonl \
  --specialize-run-plan \
  --store tests/plans/parallel-smoke.air-store.yaml \
  --output target/generated/parallel_smoke.specialized.air-plan.yaml \
  --identity-out target/generated/parallel_smoke.identity.json
cargo run -p air-cli -- validate-plan target/generated/parallel_smoke.specialized.air-plan.yaml \
  --store tests/plans/parallel-smoke.air-store.yaml

echo "[air-verify] generated retry runtime smoke"
cargo run -p air-cli -- validate-plan tests/plans/retry-smoke.air-plan.yaml \
  --store tests/plans/retry-smoke.air-store.yaml
cargo run -p air-cli -- lower-plan tests/plans/retry-smoke.air-plan.yaml \
  --store tests/plans/retry-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/retry_smoke.langgraph.py
PYTHONWARNINGS=ignore AIR_TRACE=1 "$LANGGRAPH_PYTHON" - <<'PY' > target/generated/retry_smoke.langgraph.output.json 2> target/generated/retry_smoke.langgraph.trace.jsonl
import importlib.util
import json

spec = importlib.util.spec_from_file_location("air_retry_smoke", "target/generated/retry_smoke.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

calls = {"count": 0}

def call_model(name, input_value):
    calls["count"] += 1
    if calls["count"] == 1:
        return {"summary": "invalid"}
    assert "_air_retry" in input_value
    assert input_value["_air_retry"]["attempt"] == 2
    return {"summary": "valid retry result", "rationale": "fixed schema"}

module.call_model = call_model
result = module.graph.invoke({"text": "retry this"})
assert calls["count"] == 2
assert result["result"]["summary"] == "valid retry result"
print(json.dumps(result["result"], indent=2))
PY
"$LANGGRAPH_PYTHON" - <<'PY'
import json

with open("target/generated/retry_smoke.langgraph.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]

assert any(event["action"] == "model_call_start" for event in events)
assert any(event["action"] == "model_call" and event["status"] == "error" and event["meta"]["will_retry"] is True for event in events)
assert any(event["action"] == "model_call" and event["status"] == "ok" and event["meta"]["attempt"] == 2 for event in events)
assert any(event["action"] == "return" and event["output"]["summary"] == "valid retry result" for event in events)
PY

echo "[air-verify] generated token-limit retry compaction smoke"
cargo run -p air-cli -- validate-plan tests/plans/token-limit-retry-smoke.air-plan.yaml \
  --store tests/plans/token-limit-retry-smoke.air-store.yaml
cargo run -p air-cli -- lower-plan tests/plans/token-limit-retry-smoke.air-plan.yaml \
  --store tests/plans/token-limit-retry-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/token_limit_retry_smoke.langgraph.py
PYTHONWARNINGS=ignore AIR_TRACE=1 "$LANGGRAPH_PYTHON" - <<'PY' > target/generated/token_limit_retry_smoke.langgraph.output.json 2> target/generated/token_limit_retry_smoke.langgraph.trace.jsonl
import importlib.util
import json

spec = importlib.util.spec_from_file_location("air_token_limit_retry_smoke", "target/generated/token_limit_retry_smoke.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

calls = {"count": 0}

def call_model(name, input_value):
    calls["count"] += 1
    if calls["count"] == 1:
        assert "_air_retry" not in input_value
        assert len(input_value["text"]) > 16
        raise RuntimeError("context_length_exceeded: too many tokens")
    assert input_value["_air_retry"]["reason"] == "token_limit"
    assert input_value["_air_retry"]["max_input_chars"] == 16
    assert "context_length_exceeded" in input_value["_air_retry"]["previous_error"]
    assert "[air:truncated]" in input_value["text"]
    return {"summary": "compacted retry result", "rationale": "second attempt used bounded input compaction"}

module.call_model = call_model
result = module.graph.invoke({"text": "This research context is intentionally long and should be compacted."})
assert calls["count"] == 2
assert result["result"]["summary"] == "compacted retry result"
print(json.dumps(result["result"], indent=2))
PY
"$LANGGRAPH_PYTHON" - <<'PY'
import json

with open("target/generated/token_limit_retry_smoke.langgraph.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]

assert any(event["action"] == "model_call" and event["status"] == "error" and event["meta"]["will_retry"] is True for event in events)
assert any(
    event["action"] == "model_call"
    and event["status"] == "ok"
    and event["input"]["_air_retry"]["reason"] == "token_limit"
    and event["input"]["_air_retry"]["max_input_chars"] == 16
    and "[air:truncated]" in event["input"]["text"]
    for event in events
)
PY

echo "[air-verify] generated approval runtime smoke"
cargo run -p air-cli -- validate-plan tests/plans/approval-smoke.air-plan.yaml \
  --store tests/plans/approval-smoke.air-store.yaml
cargo run -p air-cli -- run-plan tests/plans/approval-smoke.air-plan.yaml \
  --store tests/plans/approval-smoke.air-store.yaml \
  --input tests/plans/approval-smoke.input.json \
  --tool-config tests/plans/approval-smoke.tools.json \
  --trace-out target/generated/approval_smoke.native.trace.jsonl \
  > target/generated/approval_smoke.native.output.json
cargo run -p air-cli -- lower-plan tests/plans/approval-smoke.air-plan.yaml \
  --store tests/plans/approval-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/approval_smoke.openai.mjs
AIR_TRACE=1 node target/generated/approval_smoke.openai.mjs \
  --input tests/plans/approval-smoke.input.json \
  --model-config "$MODEL_CONFIG" \
  --tool-config tests/plans/approval-smoke.tools.json \
  > target/generated/approval_smoke.openai.output.json \
  2> target/generated/approval_smoke.openai.trace.jsonl
cargo run -p air-cli -- lower-plan tests/plans/approval-smoke.air-plan.yaml \
  --store tests/plans/approval-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/approval_smoke.langgraph.py
"$LANGGRAPH_PYTHON" - <<'PY' > target/generated/approval_smoke.langgraph.output.json
import importlib.util
import json

spec = importlib.util.spec_from_file_location("air_approval_smoke", "target/generated/approval_smoke.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.AIR_APPROVAL_PROVIDER = lambda **kwargs: {
    "approved": True,
    "approver": "verification",
    "reason": "approval smoke test",
}
print(json.dumps(module.graph.invoke({"deployment_id": "deploy-123"}), indent=2))
PY
"$LANGGRAPH_PYTHON" - <<'PY'
import json

with open("target/generated/approval_smoke.native.output.json", encoding="utf-8") as handle:
    native_output = json.load(handle)
assert native_output["result"]["approved"] is True

with open("target/generated/approval_smoke.openai.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
assert output["result"]["approved"] is True

with open("target/generated/approval_smoke.native.trace.jsonl", encoding="utf-8") as handle:
    native_events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "approval" and event["status"] == "ok" for event in native_events)

with open("target/generated/approval_smoke.openai.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "approval" and event["status"] == "ok" for event in events)

with open("target/generated/approval_smoke.langgraph.output.json", encoding="utf-8") as handle:
    langgraph_output = json.load(handle)
assert langgraph_output["result"]["approved"] is True
PY

echo "[air-verify] generated tool capability runtime smoke"
cargo run -p air-cli -- validate tests/agents/tool-capability-smoke.air.yaml
cargo run -p air-cli -- validate-plan tests/plans/tool-capability-smoke.air-plan.yaml \
  --store tests/plans/tool-capability-smoke.air-store.yaml
cargo run -p air-cli -- run-plan tests/plans/tool-capability-smoke.air-plan.yaml \
  --store tests/plans/tool-capability-smoke.air-store.yaml \
  --input tests/plans/tool-capability-smoke.input.json \
  --tool-config tests/plans/tool-capability-smoke.tools.json \
  --trace-out target/generated/tool_capability_smoke.native.trace.jsonl \
  > target/generated/tool_capability_smoke.native.output.json
cargo run -p air-cli -- lower-plan tests/plans/tool-capability-smoke.air-plan.yaml \
  --store tests/plans/tool-capability-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/tool_capability_smoke.openai.mjs
AIR_TRACE=1 node target/generated/tool_capability_smoke.openai.mjs \
  --input tests/plans/tool-capability-smoke.input.json \
  --model-config "$MODEL_CONFIG" \
  --tool-config tests/plans/tool-capability-smoke.tools.json \
  > target/generated/tool_capability_smoke.openai.output.json \
  2> target/generated/tool_capability_smoke.openai.trace.jsonl
cargo run -p air-cli -- lower-plan tests/plans/tool-capability-smoke.air-plan.yaml \
  --store tests/plans/tool-capability-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/tool_capability_smoke.langgraph.py
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY' > target/generated/tool_capability_smoke.langgraph.output.json
import importlib.util
import json

spec = importlib.util.spec_from_file_location("air_tool_capability_smoke", "target/generated/tool_capability_smoke.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.AIR_TOOL_CAPABILITIES = {"docs.search": "retrieval.local"}
module.AIR_TOOL_PROVIDER = lambda *, name, input_value: {
    "query": input_value["query"],
    "documents": [
        {
            "id": "refund",
            "title": "Refund policy",
            "content": "Refund requests are routed to billing support.",
        }
    ],
}
print(json.dumps(module.graph.invoke({"text": "refund policy"})["result"], indent=2))
PY
PYTHONWARNINGS=ignore "$LANGGRAPH_PYTHON" - <<'PY'
import importlib.util
import json

with open("target/generated/tool_capability_smoke.native.output.json", encoding="utf-8") as handle:
    native_output = json.load(handle)
assert native_output["result"]["query"] == "refund policy"

with open("target/generated/tool_capability_smoke.openai.output.json", encoding="utf-8") as handle:
    openai_output = json.load(handle)
assert openai_output["result"]["query"] == "refund policy"

with open("target/generated/tool_capability_smoke.openai.trace.jsonl", encoding="utf-8") as handle:
    openai_events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "tool_call" and event["status"] == "ok" for event in openai_events)

with open("target/generated/tool_capability_smoke.langgraph.output.json", encoding="utf-8") as handle:
    langgraph_output = json.load(handle)
assert langgraph_output["query"] == "refund policy"

spec = importlib.util.spec_from_file_location("air_tool_capability_smoke_bad", "target/generated/tool_capability_smoke.langgraph.py")
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

echo "[air-verify] dynamic specialization and generated backends"
cargo run -p air-cli -- validate-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml
cargo run -p air-cli -- run-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input tests/fixtures/empty.input.json \
  --trace-out target/generated/dynamic_smoke.trace.jsonl \
  > target/generated/dynamic_smoke.output.json
cargo run -p air-cli -- replay target/generated/dynamic_smoke.trace.jsonl \
  --specialize-run-plan \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --output target/generated/dynamic_smoke.specialized.air-plan.yaml \
  --identity-out target/generated/dynamic_smoke.identity.json
cargo run -p air-cli -- validate-plan target/generated/dynamic_smoke.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml
cargo run -p air-cli -- lower-plan target/generated/dynamic_smoke.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/dynamic_smoke.langgraph.py
cargo run -p air-cli -- lower-plan target/generated/dynamic_smoke.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/dynamic_smoke.openai.mjs
cargo run -p air-cli -- lower-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/dynamic_smoke.direct.langgraph.py
cargo run -p air-cli -- lower-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/dynamic_smoke.direct.openai.mjs
AIR_TRACE=1 node target/generated/dynamic_smoke.direct.openai.mjs \
  --input tests/fixtures/empty.input.json \
  --model-config "$MODEL_CONFIG" \
  > target/generated/dynamic_smoke.direct.openai.output.json \
  2> target/generated/dynamic_smoke.direct.openai.trace.jsonl
"$LANGGRAPH_PYTHON" - <<'PY'
import json

with open("target/generated/dynamic_smoke.direct.openai.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]

assert any(event["action"] == "dynamic_fanout" and event["meta"]["count"] == 2 for event in events)
assert any(event["agent"] == "final" and event["action"] == "return" and event["output"] == "done" for event in events)
PY
"$LANGGRAPH_PYTHON" - <<'PY' > target/generated/dynamic_smoke.direct.langgraph.output.json
import importlib.util
import json

spec = importlib.util.spec_from_file_location("air_dynamic_smoke", "target/generated/dynamic_smoke.direct.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
print(json.dumps(module.graph.invoke({}), indent=2))
PY

echo "[air-verify] dynamic JIT cache smoke"
rm -rf target/generated/jit_cache_smoke
cargo run -p air-cli -- run-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input tests/fixtures/empty.input.json \
  --jit-cache target/generated/jit_cache_smoke \
  > target/generated/dynamic_smoke.jit_first.output.json
cargo run -p air-cli -- run-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input tests/fixtures/empty.input.json \
  --jit-cache target/generated/jit_cache_smoke \
  --trace-out target/generated/dynamic_smoke.jit_second.trace.jsonl \
  > target/generated/dynamic_smoke.jit_second.output.json
"$LANGGRAPH_PYTHON" - <<'PY'
import json

with open("target/generated/dynamic_smoke.jit_second.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]

resolved = next(event["output"] for event in events if event["agent"] == "$planner" and event["action"] == "resolve_plan")
assert any(node["id"] == "topic_1" for node in resolved["nodes"])
assert not any(event["agent"] == "$planner" and event["action"] == "resolve_dynamic_plan" for event in events)
PY
cargo run -p air-cli -- validate-plan tests/plans/dynamic-multiple-boundaries.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml
cargo run -p air-cli -- run-plan tests/plans/dynamic-multiple-boundaries.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input tests/fixtures/empty.input.json \
  --trace-out target/generated/dynamic_multiple_boundaries.trace.jsonl \
  > target/generated/dynamic_multiple_boundaries.output.json
cargo run -p air-cli -- replay target/generated/dynamic_multiple_boundaries.trace.jsonl \
  --specialize-run-plan \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --output target/generated/dynamic_multiple_boundaries.specialized.air-plan.yaml \
  --identity-out target/generated/dynamic_multiple_boundaries.identity.json
cargo run -p air-cli -- validate-plan target/generated/dynamic_multiple_boundaries.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml
cargo run -p air-cli -- lower-plan target/generated/dynamic_multiple_boundaries.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/dynamic_multiple_boundaries.langgraph.py
cargo run -p air-cli -- lower-plan target/generated/dynamic_multiple_boundaries.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/dynamic_multiple_boundaries.openai.mjs
cargo run -p air-cli -- validate-plan tests/plans/dynamic-nested-fanout.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml
cargo run -p air-cli -- run-plan tests/plans/dynamic-nested-fanout.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input tests/fixtures/empty.input.json \
  --trace-out target/generated/dynamic_nested_fanout.trace.jsonl \
  > target/generated/dynamic_nested_fanout.output.json
cargo run -p air-cli -- replay target/generated/dynamic_nested_fanout.trace.jsonl \
  --specialize-run-plan \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --output target/generated/dynamic_nested_fanout.specialized.air-plan.yaml \
  --identity-out target/generated/dynamic_nested_fanout.identity.json
cargo run -p air-cli -- validate-plan target/generated/dynamic_nested_fanout.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml
cargo run -p air-cli -- lower-plan target/generated/dynamic_nested_fanout.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/dynamic_nested_fanout.langgraph.py
cargo run -p air-cli -- lower-plan target/generated/dynamic_nested_fanout.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/dynamic_nested_fanout.openai.mjs
cargo run -p air-cli -- lower-plan tests/plans/dynamic-nested-fanout.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend langgraph \
  --output target/generated/dynamic_nested_fanout.direct.langgraph.py
cargo run -p air-cli -- lower-plan tests/plans/dynamic-nested-fanout.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --backend openai-js-strict \
  --output target/generated/dynamic_nested_fanout.direct.openai.mjs
node target/generated/dynamic_nested_fanout.direct.openai.mjs \
  --input tests/fixtures/empty.input.json \
  --model-config "$MODEL_CONFIG" \
  > target/generated/dynamic_nested_fanout.direct.openai.output.json
"$LANGGRAPH_PYTHON" - <<'PY' > target/generated/dynamic_nested_fanout.direct.langgraph.output.json
import importlib.util
import json

spec = importlib.util.spec_from_file_location("air_dynamic_nested", "target/generated/dynamic_nested_fanout.direct.langgraph.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
print(json.dumps(module.graph.invoke({}), indent=2))
PY

echo "[air-verify] deep research AIR VM tests"
cargo test -p air-linker deep_research
cargo test -p air-cli local_docs_search_dedupes_raw_content_before_limit

if [[ "${AIR_DEEP_RESEARCH_REAL:-0}" == "1" ]]; then
  mkdir -p target/generated

  echo "[air-verify] real planner"
  cargo run -p air-cli -- plan \
    --task "Compare the commercial readiness of 800V EV platforms, silicon carbide drives, solid-state batteries, and distributed drive systems for automakers planning 2026-2030 products. Build a deep research workflow with a planning step, multiple bounded researcher steps, and a final report." \
    --store "$STORE" \
    --model-config "$MODEL_CONFIG" \
    --allow-internal \
    --output target/generated/deep_research_verify_planned.air-plan.yaml

  cargo run -p air-cli -- validate-plan target/generated/deep_research_verify_planned.air-plan.yaml --store "$STORE"

  echo "[air-verify] real AIR VM run"
  cargo run -p air-cli -- run-plan target/generated/deep_research_verify_planned.air-plan.yaml \
    --store "$STORE" \
    --input "$INPUT" \
    --model-config "$MODEL_CONFIG" \
    --tool-config "$TOOLS" \
    --trace-out target/generated/deep_research_verify.trace.jsonl \
    > target/generated/deep_research_verify.output.json

  echo "[air-verify] real dynamic AIR VM run"
  cargo run -p air-cli -- run-plan examples/deep-research/deep-research-dynamic.air-plan.yaml \
    --store "$STORE" \
    --input "$INPUT" \
    --model-config "$MODEL_CONFIG" \
    --tool-config "$TOOLS" \
    --trace-out target/generated/deep_research_dynamic_verify.trace.jsonl \
    > target/generated/deep_research_dynamic_verify.output.json
fi

echo "[air-verify] deep research verification complete"
