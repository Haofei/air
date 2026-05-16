#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STORE="examples/deep-research/module-store.air-store.yaml"
PYTHON="${AIR_PYTHON:-.venv/bin/python}"
EMPTY_INPUT="target/generated/empty.input.json"
MODEL_SMOKE_INPUT="target/generated/model-smoke.input.json"

mkdir -p target/generated
printf '{}\n' > "$EMPTY_INPUT"
cat > "$MODEL_SMOKE_INPUT" <<'JSON'
{
  "prompt": "Return a short JSON object with keys summary and risk_score. Use risk_score 0.1."
}
JSON

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

echo "[air-verify] deep research report quality contract"
"$PYTHON" - <<'PY'
import json
from pathlib import Path

model_config = json.loads(Path("examples/bigmodel-openai-compatible.json").read_text(encoding="utf-8"))
final_prompt = model_config["models"]["final_reporter"]["system_prompt"]
compressor_prompt = model_config["models"]["research_compressor"]["system_prompt"]
for needle in [
    "executive_summary",
    "comparison_matrix",
    "recommendations",
    "900-1600 words",
]:
    assert needle in final_prompt, needle
for needle in ["key_findings", "evidence", "implications", "confidence"]:
    assert needle in compressor_prompt, needle

final_schema = Path("examples/deep-research/final-report.air.yaml").read_text(encoding="utf-8")
topic_schema = Path("examples/deep-research/research-topic.air.yaml").read_text(encoding="utf-8")
for needle in ["comparison_matrix", "recommendations", "open_questions"]:
    assert needle in final_schema, needle
for needle in ["key_findings", "evidence", "implications", "gaps", "confidence"]:
    assert needle in topic_schema, needle

tools = json.loads(Path("examples/deep-research/tools.json").read_text(encoding="utf-8"))
search = tools["tools"]["web.search"]
assert search["max_results"] >= 8
assert len(search["documents"]) >= 8
PY
"$PYTHON" scripts/evaluate_deep_research_report.py \
  examples/deep-research/report-quality-output.json

echo "[air-verify] parallel CLI smoke"
cargo run -p air-cli -- validate-plan tests/plans/parallel-smoke.air-plan.yaml --store tests/plans/parallel-smoke.air-store.yaml
cargo run -p air-cli -- run-plan tests/plans/parallel-smoke.air-plan.yaml \
  --store tests/plans/parallel-smoke.air-store.yaml \
  --input "$MODEL_SMOKE_INPUT" \
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

echo "[air-verify] dynamic specialization smoke"
cargo run -p air-cli -- validate-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml
cargo run -p air-cli -- run-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input "$EMPTY_INPUT" \
  --trace-out target/generated/dynamic_smoke.trace.jsonl \
  > target/generated/dynamic_smoke.output.json
cargo run -p air-cli -- replay target/generated/dynamic_smoke.trace.jsonl \
  --specialize-run-plan \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --output target/generated/dynamic_smoke.specialized.air-plan.yaml \
  --identity-out target/generated/dynamic_smoke.identity.json
cargo run -p air-cli -- validate-plan target/generated/dynamic_smoke.specialized.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml

echo "[air-verify] dynamic JIT cache smoke"
cargo run -p air-cli -- run-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input "$EMPTY_INPUT" \
  --jit-cache target/generated/jit_cache_smoke \
  > target/generated/dynamic_smoke.jit_first.output.json
cargo run -p air-cli -- run-plan tests/plans/dynamic-smoke.air-plan.yaml \
  --store tests/plans/dynamic-smoke.air-store.yaml \
  --input "$EMPTY_INPUT" \
  --jit-cache target/generated/jit_cache_smoke \
  --trace-out target/generated/dynamic_smoke.jit_second.trace.jsonl \
  > target/generated/dynamic_smoke.jit_second.output.json

echo "[air-verify] deep research AIR VM tests"
cargo test -p air-linker deep_research -- --nocapture
cargo test -p air-cli local_docs_search_dedupes_raw_content_before_limit

echo "[air-verify] deep research verification complete"
