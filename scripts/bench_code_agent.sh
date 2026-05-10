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
  "Fix a failing test using structured diagnostics, apply bounded file operations, and retest." \
  "code.core_repair@0.1.0" \
  "recipe"
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

cargo run -q -p air-cli -- run-plan examples/code-agent/code-repair-with-explore.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/repair-core.input.json \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code-agent-bench/repair.trace.jsonl \
  > target/generated/code-agent-bench/repair.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code-agent-bench/repair.post_test.log
restore_fixture
trap - EXIT

echo "[code-agent-bench] offline project recovery repair"
cat > target/generated/code-agent-bench/project_fail_model_fixtures.json <<'JSON'
{
  "fixtures": {
    "project_planner": {
      "summary": "Benchmark project plan intentionally fails acceptance before recovery.",
      "scope": {
        "goal": "Exercise project recovery from a stopped AIR code-agent run.",
        "non_goals": [],
        "assumptions": ["The first turn should stop on a missing acceptance alias."]
      },
      "milestones": [
        {
          "id": "m1",
          "title": "Failure turn",
          "objective": "Create a stopped project turn with recovery metadata.",
          "task_ids": ["t1"]
        }
      ],
      "tasks": [
        {
          "id": "t1",
          "title": "Explore then fail acceptance",
          "description": "Run a read-only task, then fail an allowlisted acceptance check.",
          "kind": "test",
          "recipe": "explore",
          "input": {
            "task": "Inspect code-agent project recovery behavior.",
            "query": "code agent project recovery",
            "target_path": "crates/air-cli/src/code_agent.rs"
          },
          "depends_on": [],
          "files": ["crates/air-cli/src/code_agent.rs"],
          "acceptance": [
            {
              "id": "missing-alias",
              "command": "missing_acceptance_alias",
              "expected": "This intentionally fails so recovery metadata is produced."
            }
          ]
        }
      ],
      "files": [
        {
          "path": "crates/air-cli/src/code_agent.rs",
          "purpose": "Project recovery implementation.",
          "change_type": "modify"
        }
      ],
      "acceptance": [],
      "risks": [],
      "source_ids": ["docs-code-agent-loop"],
      "next_steps": ["Run a recovery repair plan."]
    },
    "code_explorer": {
      "summary": "Benchmark exploration completed before acceptance failure.",
      "relevant_files": [
        {
          "path": "crates/air-cli/src/code_agent.rs",
          "reason": "Contains project recovery behavior."
        }
      ],
      "findings": [
        {
          "title": "Recovery is session-scoped",
          "evidence": "A stopped project turn should include recovery metadata and a fork.",
          "source_ids": ["docs-provenance"]
        }
      ],
      "source_ids": ["docs-provenance"],
      "next_steps": ["Use the recovery context to plan a fix."]
    }
  }
}
JSON
cat > target/generated/code-agent-bench/project_recover_model_fixtures.json <<'JSON'
{
  "fixtures": {
    "project_planner": {
      "summary": "Benchmark recovery project plan runs a bounded repair task.",
      "scope": {
        "goal": "Recover from the stopped project execution by repairing the failing fixture.",
        "non_goals": [],
        "assumptions": ["The previous turn provided failed task and acceptance context."]
      },
      "milestones": [
        {
          "id": "m1",
          "title": "Recovery repair",
          "objective": "Run the repair recipe against the failed fixture.",
          "task_ids": ["fix-t1"]
        }
      ],
      "tasks": [
        {
          "id": "fix-t1",
          "title": "Repair failed project task",
          "description": "Use recovery context to repair the failing math fixture and retest it.",
          "kind": "repair",
          "recipe": "repair",
          "input": {
            "task": "Repair the failing add implementation from the prior project recovery context.",
            "query": "repair fixture add function",
            "target_path": "examples/code-agent/repair-fixture/math.js",
            "related_files": ["examples/code-agent/repair-fixture/test.js"],
            "test_command": "repair_fixture_test"
          },
          "depends_on": [],
          "files": ["examples/code-agent/repair-fixture/math.js"],
          "acceptance": []
        }
      ],
      "files": [
        {
          "path": "examples/code-agent/repair-fixture/math.js",
          "purpose": "Fix the failed project task.",
          "change_type": "modify"
        }
      ],
      "acceptance": [],
      "risks": [],
      "source_ids": ["docs-code-agent-loop"],
      "next_steps": ["Verify the repair output."]
    },
    "code_explorer": {
      "summary": "Benchmark exploration completed for recovery repair.",
      "relevant_files": [
        {
          "path": "examples/code-agent/repair-fixture/math.js",
          "reason": "The failed implementation under repair."
        },
        {
          "path": "examples/code-agent/repair-fixture/test.js",
          "reason": "The allowlisted test documents expected add behavior."
        }
      ],
      "findings": [
        {
          "title": "add should sum its operands",
          "evidence": "The repair fixture expects add(2, 3) to equal 5.",
          "source_ids": ["docs-repair-safety"]
        }
      ],
      "source_ids": ["docs-repair-safety"],
      "next_steps": ["Patch math.js and rerun repair_fixture_test."]
    },
    "code_repair_context_selector": {
      "target_path": "examples/code-agent/repair-fixture/math.js",
      "related_files": ["examples/code-agent/repair-fixture/test.js"],
      "rationale": "The test file is the minimal related context needed to repair add."
    },
    "code_repairer": {
      "operations": [
        {
          "kind": "edit",
          "path": "examples/code-agent/repair-fixture/math.js",
          "old_string": "function add(a, b) {\n  return a - b;\n}",
          "new_string": "function add(a, b) {\n  return a + b;\n}"
        }
      ],
      "rationale": "The failed diagnostic shows add subtracts instead of summing; replace subtraction with addition."
    }
  }
}
JSON
project_repair_backup="$(mktemp)"
cp "$repair_fixture" "$project_repair_backup"
restore_project_repair_fixture() {
  cp "$project_repair_backup" "$repair_fixture"
  rm -f "$project_repair_backup"
}
trap restore_project_repair_fixture EXIT
project_session="target/generated/code-agent-bench/project_recovery.session.json"
rm -f "$project_session" target/generated/code-agent-bench/project_recovery.first.output.json target/generated/code-agent-bench/project_recovery.second.output.json
rm -rf target/generated/code-agent-bench/project_recovery.session.traces target/generated/code-agent-bench/project_recovery.session.forks
cargo run -q -p air-cli -- code "plan a project that should stop before recovery" \
  --recipe plan \
  --execute-plan \
  --max-iterations 1 \
  --query "code agent project recovery" \
  --model-config target/generated/code-agent-bench/project_fail_model_fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --session "$project_session" \
  > target/generated/code-agent-bench/project_recovery.first.output.json
cargo run -q -p air-cli -- code "recover and repair the failed project execution" \
  --recipe plan \
  --execute-plan \
  --max-iterations 1 \
  --query "code agent project recovery" \
  --model-config target/generated/code-agent-bench/project_recover_model_fixtures.json \
  --tool-config examples/code-agent/tools.core.json \
  --session "$project_session" \
  > target/generated/code-agent-bench/project_recovery.second.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code-agent-bench/project_recovery.post_test.log
restore_project_repair_fixture
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
    repair_output = json.load(handle)
exploration = repair_output["exploration"]
repair_context = repair_output["repair_context"]
repair = repair_output["repair"]
with (root / "repair.trace.jsonl").open(encoding="utf-8") as handle:
    repair_trace = [json.loads(line) for line in handle if line.strip()]
with (root / "project_recovery.first.output.json").open(encoding="utf-8") as handle:
    project_first = json.load(handle)
with (root / "project_recovery.second.output.json").open(encoding="utf-8") as handle:
    project_second = json.load(handle)
with (root / "project_recovery.session.json").open(encoding="utf-8") as handle:
    project_session = json.load(handle)

def trace_counts(events):
    counts = {"model_calls": 0, "tool_calls": 0, "approval_calls": 0}
    tools = {}
    models = {}
    for event in events:
        action = event.get("action")
        meta = event.get("meta", {})
        if action == "model_call":
            counts["model_calls"] += 1
            model = meta.get("model", "unknown")
            models[model] = models.get(model, 0) + 1
        elif action == "tool_call":
            counts["tool_calls"] += 1
            tool = meta.get("tool", "unknown")
            tools[tool] = tools.get(tool, 0) + 1
        elif action == "approval":
            counts["approval_calls"] += 1
    counts["models"] = dict(sorted(models.items()))
    counts["tools"] = dict(sorted(tools.items()))
    return counts

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
assert exploration["relevant_files"], exploration
assert repair_context["related_files"], repair_context
changed_paths = [entry["path"] for entry in repair["changed_files"]]
assert repair["target_path"] in changed_paths, repair
assert repair["target_path"] in repair["workspace_diff"]["diff"], repair
assert any(
    event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_repair_context_selector"
    and event.get("status") == "ok"
    for event in repair_trace
), repair_trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.read_many"
    and event.get("status") == "ok"
    for event in repair_trace
), repair_trace
assert project_first["project"]["status"] == "stopped", project_first
assert project_first["recovery"]["failed_task_id"] == "t1", project_first
project_recovery = project_second["project"]
assert project_recovery["status"] == "completed", project_recovery
assert project_recovery["completed"] is True, project_recovery
assert project_recovery["executed_this_run"] == 1, project_recovery
assert [item["task_id"] for item in project_recovery["executions"]] == ["fix-t1"], project_recovery
assert project_recovery["budget"]["task_count"] == 1, project_recovery
assert project_recovery["budget"]["max_estimated_model_calls"] > 0, project_recovery
assert project_recovery["budget"]["max_estimated_tool_calls"] > 0, project_recovery
assert project_recovery["executions"][0]["budget"]["recipe"] == "repair", project_recovery
project_repair = project_recovery["executions"][0]["outputs"]["repair"]
assert project_repair["final_success"] is True, project_repair
assert project_repair["patch_applied"] is True, project_repair
assert "examples/code-agent/repair-fixture/math.js" in project_repair["workspace_diff"]["diff"], project_repair
assert len(project_session["turns"]) == 2, project_session
assert project_session["turns"][0]["outputs"]["project"]["status"] == "stopped", project_session
assert project_session["turns"][1]["outputs"]["project"]["status"] == "completed", project_session
assert "AIR project recovery context" in project_session["turns"][1]["input"]["task"], project_session

route_cases = [
    {
        "id": f"route.{name}",
        "status": "passed",
        "expected": {"component": first["id"], "source": first["source"]},
        "actual": {
            "component": first["id"],
            "source": first["source"],
            "score": first["score"],
            "matched_terms": first["matched_terms"],
        },
        "artifacts": [str(root / f"route_{name}.json")],
    }
    for name, first in sorted(routes.items())
]

cases = route_cases + [
    {
        "id": "review.grounded",
        "status": "passed",
        "metrics": {
            "findings": len(review["findings"]),
            "search_sufficient": review["search_quality"]["sufficient"],
        },
        "artifacts": [str(root / "review.output.json")],
    },
    {
        "id": "explore.repository_context",
        "status": "passed",
        "metrics": {
            "relevant_files": len(explore["relevant_files"]),
            "findings": len(explore["findings"]),
            "has_source_ids": bool(explore["findings"][0]["source_ids"]),
        },
        "artifacts": [str(root / "explore.output.json")],
    },
    {
        "id": "build.static_page",
        "status": "passed",
        "metrics": {
            "bytes": build["bytes"],
            "test_success": build["test_success"],
            "audit_success": build["audit_success"],
            "screenshots": len(build["screenshots"]),
            "revised": build["revised"],
            **trace_counts(build_trace),
        },
        "artifacts": [
            str(root / "build.output.json"),
            str(root / "build.trace.jsonl"),
        ],
    },
    {
        "id": "repair.explore_patch_retest",
        "status": "passed",
        "metrics": {
            "initial_success": repair["initial_success"],
            "final_success": repair["final_success"],
            "patch_applied": repair["patch_applied"],
            "workspace_clean_before": repair["workspace_clean_before"],
            "target_changed": repair["target_path"] in changed_paths,
            "workspace_diff_bytes": repair["workspace_diff"]["bytes"],
            "workspace_diff_truncated": repair["workspace_diff"]["truncated"],
            "preexisting_changed_files": len(repair["preexisting_changed_files"]),
            "workspace_changed_files": len(repair["workspace_changed_files"]),
            "explored_files": len(exploration["relevant_files"]),
            "selected_related_files": len(repair_context["related_files"]),
            **trace_counts(repair_trace),
        },
        "artifacts": [
            str(root / "repair.output.json"),
            str(root / "repair.trace.jsonl"),
            str(root / "repair.post_test.log"),
        ],
    },
    {
        "id": "project.recovery_replan_repair",
        "status": "passed",
        "metrics": {
            "first_status": project_first["project"]["status"],
            "second_status": project_recovery["status"],
            "turns": len(project_session["turns"]),
            "recovery_context_injected": "AIR project recovery context"
            in project_session["turns"][1]["input"]["task"],
            "executed_this_run": project_recovery["executed_this_run"],
            "max_estimated_model_calls": project_recovery["budget"][
                "max_estimated_model_calls"
            ],
            "max_estimated_tool_calls": project_recovery["budget"][
                "max_estimated_tool_calls"
            ],
            "repair_final_success": project_repair["final_success"],
            "repair_patch_applied": project_repair["patch_applied"],
            "workspace_diff_bytes": project_repair["workspace_diff"]["bytes"],
        },
        "artifacts": [
            str(root / "project_recovery.first.output.json"),
            str(root / "project_recovery.second.output.json"),
            str(root / "project_recovery.session.json"),
            str(root / "project_recovery.post_test.log"),
        ],
    },
]

summary = {
    "schema_version": 1,
    "bench": "code-agent-offline",
    "status": "passed",
    "case_count": len(cases),
    "cases": cases,
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
        "workspace_clean_before": repair["workspace_clean_before"],
        "target_changed": repair["target_path"] in changed_paths,
        "preexisting_changed_files": len(repair["preexisting_changed_files"]),
        "workspace_changed_files": len(repair["workspace_changed_files"]),
        "explored_files": len(exploration["relevant_files"]),
        "selected_related_files": len(repair_context["related_files"]),
    },
    "project_recovery": {
        "first_status": project_first["project"]["status"],
        "second_status": project_recovery["status"],
        "turns": len(project_session["turns"]),
        "recovery_context_injected": "AIR project recovery context"
        in project_session["turns"][1]["input"]["task"],
        "repair_final_success": project_repair["final_success"],
        "repair_patch_applied": project_repair["patch_applied"],
        "max_estimated_model_calls": project_recovery["budget"][
            "max_estimated_model_calls"
        ],
        "max_estimated_tool_calls": project_recovery["budget"][
            "max_estimated_tool_calls"
        ],
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
assert summary["status"] == "passed"
assert summary["case_count"] == len(summary["cases"])
assert all(case["status"] == "passed" for case in summary["cases"])
PY

echo "[code-agent-bench] summary: target/generated/code-agent-bench/summary.json"
