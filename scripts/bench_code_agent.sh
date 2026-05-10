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
assert first["tier"] == "large_component", first
assert all(candidate["matched_terms"] for candidate in output["component_selection"]["recommended"])
PY
}

echo "[code-agent-bench] route primary task shapes"
run_route \
  "review" \
  "Review the command_run implementation for safety, provenance, and diagnostics." \
  "code.review_with_std_context@0.1.0" \
  "recipe"
run_route \
  "repair" \
  "Fix a failing test using structured diagnostics, apply a bounded patch, and retest." \
  "code.repair@0.1.0" \
  "module"
run_route \
  "build" \
  "Build an Apple-style landing page as a single HTML file and verify screenshots." \
  "code.build_page@0.1.0" \
  "module"
run_route \
  "explore" \
  "Explore how command_run is implemented and identify relevant repository files." \
  "code.explore@0.1.0" \
  "module"

echo "[code-agent-bench] offline review"
cargo run -q -p air-cli -- run-plan examples/code-agent/code-review-composed.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/input.json \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  > target/generated/code-agent-bench/review.output.json

echo "[code-agent-bench] offline explore"
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/explore.air-profile.yaml \
  > target/generated/code-agent-bench/explore.output.json

echo "[code-agent-bench] offline build"
build_output="examples/apple-landing/index.html"
build_backup="$(mktemp)"
build_had_output=0
if [[ -f "$build_output" ]]; then
  cp "$build_output" "$build_backup"
  build_had_output=1
fi
restore_build_output() {
  if [[ "$build_had_output" == "1" ]]; then
    mkdir -p "$(dirname "$build_output")"
    cp "$build_backup" "$build_output"
  else
    rm -f "$build_output"
  fi
  rm -f "$build_backup"
}
trap restore_build_output EXIT

cargo run -q -p air-cli -- run-plan examples/code-agent/code-build-page.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/apple-landing.input.json \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.build.json \
  --trace-out target/generated/code-agent-bench/build.trace.jsonl \
  > target/generated/code-agent-bench/build.output.json
restore_build_output
trap - EXIT

echo "[code-agent-bench] offline repair"
repair_fixture="examples/code-agent/repair-fixture/math.js"
repair_fixture_backup="$(mktemp)"
cp "$repair_fixture" "$repair_fixture_backup"
restore_fixture() {
  cp "$repair_fixture_backup" "$repair_fixture"
  rm -f "$repair_fixture_backup"
}
trap restore_fixture EXIT

cargo run -q -p air-cli -- run-plan examples/code-agent/code-repair.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/repair.input.json \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.repair.json \
  --trace-out target/generated/code-agent-bench/repair.trace.jsonl \
  > target/generated/code-agent-bench/repair.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code-agent-bench/repair.post_test.log
restore_fixture
trap - EXIT

if [[ "${AIR_CODE_AGENT_BENCH_REAL_BUILD:-0}" == "1" ]]; then
  echo "[code-agent-bench] real build"
  cargo run -q -p air-cli -- run-plan --profile examples/code-agent/apple-build.air-profile.yaml --log \
    > target/generated/code-agent-bench/build.real.output.json
fi

"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

root = Path("target/generated/code-agent-bench")

routes = {}
for path in sorted(root.glob("route_*.json")):
    with path.open(encoding="utf-8") as handle:
        output = json.load(handle)
    routes[path.stem.removeprefix("route_")] = output["component_selection"]["first_choice"]

with (root / "review.output.json").open(encoding="utf-8") as handle:
    review = json.load(handle)["review"]
with (root / "explore.output.json").open(encoding="utf-8") as handle:
    explore = json.load(handle)["exploration"]
with (root / "build.output.json").open(encoding="utf-8") as handle:
    build = json.load(handle)["build"]
with (root / "build.trace.jsonl").open(encoding="utf-8") as handle:
    build_trace = [json.loads(line) for line in handle if line.strip()]
with (root / "repair.output.json").open(encoding="utf-8") as handle:
    repair = json.load(handle)["repair"]
with (root / "repair.trace.jsonl").open(encoding="utf-8") as handle:
    repair_trace = [json.loads(line) for line in handle if line.strip()]

assert review["search_quality"]["sufficient"] is True
assert review["findings"], review
assert explore["relevant_files"], explore
assert explore["findings"][0]["source_ids"], explore
assert build["test_success"] is True, build
assert build["audit_success"] is True, build
assert build["revised"] is False, build
assert len(build["screenshots"]) >= 2, build
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "browser.audit"
    and event.get("status") == "ok"
    for event in build_trace
), build_trace
assert repair["initial_success"] is False, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
changed_paths = [entry["path"] for entry in repair["changed_files"]]
assert repair["target_path"] in changed_paths, repair
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.read_many"
    and event.get("status") == "ok"
    for event in repair_trace
), repair_trace

summary = {
    "routes": routes,
    "review": {
        "summary": review["summary"],
        "findings": len(review["findings"]),
        "search_sufficient": review["search_quality"]["sufficient"],
    },
    "explore": {
        "summary": explore["summary"],
        "relevant_files": len(explore["relevant_files"]),
        "findings": len(explore["findings"]),
    },
    "build": {
        "path": build["path"],
        "bytes": build["bytes"],
        "test_success": build["test_success"],
        "audit_success": build["audit_success"],
        "screenshots": len(build["screenshots"]),
        "revised": build["revised"],
    },
    "repair": {
        "initial_success": repair["initial_success"],
        "final_success": repair["final_success"],
        "patch_applied": repair["patch_applied"],
        "target_changed": repair["target_path"] in changed_paths,
        "workspace_changed_files": len(changed_paths),
    },
}

if (root / "build.real.output.json").exists():
    with (root / "build.real.output.json").open(encoding="utf-8") as handle:
        real_build = json.load(handle)["build"]
    summary["build_real"] = {
        "test_success": real_build["test_success"],
        "audit_success": real_build["audit_success"],
        "path": real_build["path"],
        "revised": real_build["revised"],
        "screenshots": len(real_build["screenshots"]),
    }

with (root / "summary.json").open("w", encoding="utf-8") as handle:
    json.dump(summary, handle, indent=2)
    handle.write("\n")
print(json.dumps(summary, indent=2))
PY

echo "[code-agent-bench] summary: target/generated/code-agent-bench/summary.json"
