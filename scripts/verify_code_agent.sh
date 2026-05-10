#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p target/generated

echo "[code-agent] validate packaged profiles"
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/profile.air-profile.yaml
cargo run -q -p air-cli -- validate-plan examples/code-agent/code-review.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml
cargo run -q -p air-cli -- validate-plan examples/code-agent/code-review-composed.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/project-plan.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/explore.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/dynamic-explore.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/apple-build.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/repair.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/repair-core.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/repair-multifile.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/refactor-core.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/code-agent/open-refactor.air-profile.yaml
cargo test -q -p air-cli code_agent_pack_declares_all_default_profiles

echo "[code-agent] deterministic tool coverage"
node --check scripts/playwright_search.cjs
node --check scripts/playwright_search_fixture_test.cjs
node --check scripts/playwright_page_audit.cjs
node --check scripts/playwright_page_audit_fixture_test.cjs
node scripts/playwright_search_fixture_test.cjs
node scripts/playwright_page_audit_fixture_test.cjs
cargo test -q -p air-tools file_read
cargo test -q -p air-tools file_read_many
cargo test -q -p air-tools file_patch
cargo test -q -p air-tools file_edit
cargo test -q -p air-tools file_ops
cargo test -q -p air-tools stale_read
cargo test -q -p air-tools git_status
cargo test -q -p air-tools todo_write
cargo test -q -p air-tools todo_read
cargo test -q -p air-tools context_measure
cargo test -q -p air-tools playwright_page_audit
cargo test -q -p air-tools repo_search
cargo test -q -p air-tools repo_symbols
cargo test -q -p air-tools repo_references
cargo test -q -p air-tools repo_context
cargo test -q -p air-tools diagnostic_context
cargo test -q -p air-tools command_run
cargo test -q -p air-tools extract_command_diagnostics_parses_rustc_location_blocks
cargo test -q -p air-tools extract_command_diagnostics_parses_python_tracebacks
cargo test -q -p air-tools extract_command_diagnostics_parses_line_only_colon_diagnostics
cargo test -q -p air-tools extract_command_diagnostics_parses_file_context_lint_blocks
cargo test -q -p air-cli model_config_prompts_match_code_agent_schemas

echo "[code-agent] composed review offline run"
cargo run -q -p air-cli -- run-plan examples/code-agent/code-review-composed.air-plan.yaml \
  --store examples/code-agent/module-store.air-store.yaml \
  --input examples/code-agent/input.json \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --trace-out target/generated/code_review_composed_fixture.trace.jsonl \
  > target/generated/code_review_composed_fixture.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_review_composed_fixture.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
review = output["review"]
assert review["summary"].startswith("Fixture review completed")
assert review["findings"][0]["severity"] == "info"
assert review["search_quality"]["sufficient"] is True
with open("target/generated/code_review_composed_fixture.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
checks = [
    event for event in events
    if event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "artifact.validate"
]
assert checks, "expected artifact.validate trace event"
assert checks[-1]["output"]["valid"] is True, checks[-1]
PY

echo "[code-agent] user-facing review command offline run"
cargo run -q -p air-cli -- code "review the Playwright search tool" \
  --recipe review \
  --target scripts/playwright_search.cjs \
  --query playwright_search \
  --search-query "Playwright browser search result extraction timeout Node.js" \
  --required-term playwright \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  > target/generated/code_command_review.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_review.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
review = output["review"]
assert review["summary"].startswith("Fixture review completed")
assert review["search_quality"]["sufficient"] is True
PY

echo "[code-agent] project plan offline run"
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/project-plan.air-profile.yaml \
  --trace-out target/generated/code_project_plan_fixture.trace.jsonl \
  > target/generated/code_project_plan_fixture.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_project_plan_fixture.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
plan = output["project_plan"]
assert plan["summary"].startswith("Fixture project plan completed")
assert plan["tasks"], plan
assert plan["acceptance"], plan
assert "docs-code-agent-loop" in plan["source_ids"], plan

with open("target/generated/code_project_plan_fixture.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
tools = {
    event.get("meta", {}).get("tool")
    for event in events
    if event.get("action") == "tool_call"
}
assert {"repo.files", "repo.search", "repo.context"} <= tools, tools
models = {
    event.get("meta", {}).get("model")
    for event in events
    if event.get("action") == "model_call"
}
assert "project_planner" in models, models
PY

echo "[code-agent] user-facing project plan command offline run"
rm -f target/generated/code_project_plan.session.json
rm -rf target/generated/code_project_plan.session.traces
cargo run -q -p air-cli -- code "plan a project-level task graph for the AIR code agent" \
  --recipe plan \
  --query "code agent project plan task graph" \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --session target/generated/code_project_plan.session.json \
  > target/generated/code_command_project_plan.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_project_plan.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
plan = output["project_plan"]
assert plan["tasks"], plan
assert plan["acceptance"], plan

with open("target/generated/code_project_plan.session.json", encoding="utf-8") as handle:
    session = json.load(handle)
turn = session["turns"][-1]
assert turn["recipe"] == "plan", turn
assert turn["pack"]["path"] == "examples/code-agent/code-agent.air-pack.yaml", turn
assert turn["pack"]["recipe"] == "plan", turn
assert turn["pack"]["default_profile"] == "examples/code-agent/project-plan.air-profile.yaml", turn
assert turn["pack"]["profile_override"] is False, turn
assert turn["completed"] is True, turn
assert turn["summary"]["model_call_count"] >= 1, turn
assert "project_planner" in turn["summary"]["models"], turn["summary"]
PY

echo "[code-agent] project plan execution offline run"
cargo run -q -p air-cli -- code "plan and start executing a project-level task graph for the AIR code agent" \
  --recipe plan \
  --execute-plan \
  --max-iterations 2 \
  --query "code agent project plan task graph" \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --trace-out target/generated/code_project_execute.trace.jsonl \
  > target/generated/code_project_execute.output.json
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_project_execute.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
project = output["project"]
assert project["status"] == "max_tasks_exhausted", project
assert project["completed"] is False, project
assert project["executed_tasks"] == 2, project
assert [item["recipe"] for item in project["executions"]] == ["explore", "explore"], project
assert all(item["completed"] for item in project["executions"]), project
assert all(item["acceptance"] for item in project["executions"]), project
assert all(item["acceptance"][0]["success"] for item in project["executions"]), project
budget = project["budget"]
assert budget["task_count"] == len(project["scheduled_task_ids"]), budget
assert budget["max_estimated_model_calls"] > 0, budget
assert budget["max_estimated_tool_calls"] > 0, budget
assert len(budget["tasks"]) == len(project["scheduled_task_ids"]), budget
assert [item["task_id"] for item in budget["tasks"]] == project["scheduled_task_ids"], budget
assert all(item["budget"]["max_estimated_model_calls"] > 0 for item in project["executions"]), project
assert all(item["budget"]["max_estimated_tool_calls"] > 0 for item in project["executions"]), project
memory = project["memory"]
assert memory["task_count"] == 2, memory
assert memory["completed_task_count"] == 2, memory
assert [item["task_id"] for item in memory["tasks"]] == ["t1", "t2"], memory
assert "exploration" in memory["tasks"][0]["outputs"], memory
artifacts = project["artifacts"]
artifact_ids = [item["id"] for item in artifacts]
assert any(item["kind"] == "trace_jsonl" for item in artifacts), artifacts
assert any(item.startswith("trace:") and item.endswith(".task1.jsonl") for item in artifact_ids), artifacts
trace_files = [Path(path) for path in output["trace_files"]]
assert len(trace_files) == 4, trace_files
assert any(path.name.endswith(".acceptance.jsonl") for path in trace_files), trace_files
for path in trace_files:
    assert path.exists(), path
with trace_files[0].open(encoding="utf-8") as handle:
    plan_events = [json.loads(line) for line in handle if line.strip()]
with trace_files[1].open(encoding="utf-8") as handle:
    task_events = [json.loads(line) for line in handle if line.strip()]
with trace_files[2].open(encoding="utf-8") as handle:
    task2_events = [json.loads(line) for line in handle if line.strip()]
acceptance_trace = next(path for path in trace_files if path.name.endswith(".acceptance.jsonl"))
with acceptance_trace.open(encoding="utf-8") as handle:
    acceptance_events = [json.loads(line) for line in handle if line.strip()]
assert any(event.get("meta", {}).get("model") == "project_planner" for event in plan_events), plan_events
assert any(event.get("meta", {}).get("model") == "code_explorer" for event in task_events), task_events
assert any("AIR project memory from previous tasks" in json.dumps(event.get("input", {})) for event in task2_events), task2_events
assert any("Available project artifacts from previous tasks" in json.dumps(event.get("input", {})) for event in task2_events), task2_events
assert any(event.get("meta", {}).get("tool") == "test.run" for event in acceptance_events), acceptance_events
PY

echo "[code-agent] project plan executes repair tasks offline"
cat > target/generated/code_project_repair_model_fixtures.json <<'JSON'
{
  "fixtures": {
    "project_planner": {
      "summary": "Fixture project repair plan completed: execute one bounded repair task and verify the failing fixture.",
      "scope": {
        "goal": "Repair the add function in the code-agent repair fixture and confirm the allowlisted test passes.",
        "non_goals": [
          "Do not change unrelated fixtures.",
          "Do not run raw shell commands outside configured test aliases."
        ],
        "assumptions": [
          "The failing implementation lives in examples/code-agent/repair-fixture/math.js.",
          "The repair_fixture_test command is configured in the code-agent tool config."
        ]
      },
      "milestones": [
        {
          "id": "m1",
          "title": "Repair fixture",
          "objective": "Patch the failing add implementation and retest it.",
          "task_ids": [
            "repair-add"
          ]
        }
      ],
      "tasks": [
        {
          "id": "repair-add",
          "title": "Fix add and retest",
          "description": "Replace subtraction with addition in the repair fixture and run the allowlisted test command.",
          "kind": "repair",
          "recipe": "repair",
          "input": {
            "task": "Fix the failing add function and retest.",
            "query": "repair fixture add function",
            "target_path": "examples/code-agent/repair-fixture/math.js",
            "related_files": [
              "examples/code-agent/repair-fixture/test.js"
            ],
            "test_command": "repair_fixture_test"
          },
          "depends_on": [],
          "files": [
            "examples/code-agent/repair-fixture/math.js"
          ],
          "acceptance": [
            {
              "id": "repair-test",
              "command": "repair_fixture_test",
              "expected": "The repair fixture test passes after the patch."
            }
          ]
        }
      ],
      "files": [
        {
          "path": "examples/code-agent/repair-fixture/math.js",
          "purpose": "Fix the add implementation.",
          "change_type": "modify"
        }
      ],
      "acceptance": [
        {
          "id": "repair-test",
          "command": "repair_fixture_test",
          "expected": "The repair fixture test passes after the patch."
        }
      ],
      "risks": [
        {
          "risk": "A project plan could schedule only read-only exploration and never exercise the repair executor.",
          "mitigation": "This fixture schedules an explicit repair recipe and asserts patch plus retest trace events."
        }
      ],
      "source_ids": [
        "docs-repair-safety"
      ],
      "next_steps": [
        "Use this fixture to keep project execution wired to mutating repair tasks."
      ]
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
      "rationale": "The failing fixture subtracts instead of adding. Replace the operator and keep the module export unchanged."
    }
  }
}
JSON
repair_fixture_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$repair_fixture_backup"
restore_project_repair_fixture() {
  cp "$repair_fixture_backup" examples/code-agent/repair-fixture/math.js
  rm -f "$repair_fixture_backup"
}
trap restore_project_repair_fixture EXIT
cargo run -q -p air-cli -- code "plan and execute a project repair task for the AIR code agent" \
  --recipe plan \
  --execute-plan \
  --max-iterations 1 \
  --query "repair fixture add function" \
  --model-config target/generated/code_project_repair_model_fixtures.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_project_repair_execute.trace.jsonl \
  > target/generated/code_project_repair_execute.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code_project_repair_execute.post_test.log
restore_project_repair_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_project_repair_execute.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
project = output["project"]
assert project["status"] == "completed", project
assert project["completed"] is True, project
assert project["executed_this_run"] == 1, project
assert len(project["executions"]) == 1, project
execution = project["executions"][0]
assert execution["task_id"] == "repair-add", execution
assert execution["recipe"] == "repair", execution
assert execution["completed"] is True, execution
repair = execution["outputs"]["repair"]
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert repair["workspace_diff"]["diff"], repair
assert "examples/code-agent/repair-fixture/math.js" in repair["workspace_diff"]["diff"], repair
trace_files = [Path(path) for path in output["trace_files"]]
assert any(path.name.endswith(".plan.jsonl") for path in trace_files), trace_files
assert any(path.name.endswith(".task1.jsonl") for path in trace_files), trace_files
plan_trace = next(path for path in trace_files if path.name.endswith(".plan.jsonl"))
task_trace = next(path for path in trace_files if path.name.endswith(".task1.jsonl"))
with plan_trace.open(encoding="utf-8") as handle:
    plan_events = [json.loads(line) for line in handle if line.strip()]
with task_trace.open(encoding="utf-8") as handle:
    task_events = [json.loads(line) for line in handle if line.strip()]
assert any(event.get("meta", {}).get("model") == "project_planner" for event in plan_events), plan_events
assert any(event.get("meta", {}).get("model") == "code_repairer" for event in task_events), task_events
assert any(event.get("meta", {}).get("tool") == "file.ops" for event in task_events), task_events
assert any(event.get("meta", {}).get("tool") == "test.run" for event in task_events), task_events
with open("target/generated/code_project_repair_execute.post_test.log", encoding="utf-8") as handle:
    assert "ok repair fixture" in handle.read()
PY

echo "[code-agent] project execution respects estimated budget limits"
cargo run -q -p air-cli -- code "plan and start executing a project-level task graph for the AIR code agent" \
  --recipe plan \
  --execute-plan \
  --max-iterations 2 \
  --max-estimated-model-calls 1 \
  --query "code agent project plan task graph" \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --trace-out target/generated/code_project_budget_limit.trace.jsonl \
  > target/generated/code_project_budget_limit.output.json
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_project_budget_limit.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
project = output["project"]
assert project["status"] == "budget_exceeded", project
assert project["completed"] is False, project
assert project["executed_this_run"] == 0, project
assert project["executions"] == [], project
assert project["budget_limit"]["exceeded"] is True, project
assert project["budget_limit"]["max_estimated_model_calls"] == 1, project
assert project["budget_limit"]["model_exceeded"] is True, project
assert project["remaining_budget"]["max_estimated_model_calls"] > 1, project
assert project["remaining_task_ids"] == project["scheduled_task_ids"], project
trace_files = [Path(path) for path in output["trace_files"]]
assert len(trace_files) == 1, trace_files
assert trace_files[0].name.endswith(".plan.jsonl"), trace_files
assert trace_files[0].exists(), trace_files
PY

echo "[code-agent] project execution resumes exhausted sessions"
rm -f target/generated/code_project_resume.session.json \
  target/generated/code_project_resume.first.output.json \
  target/generated/code_project_resume.second.output.json
rm -rf target/generated/code_project_resume.session.traces
cargo run -q -p air-cli -- code "plan and execute a project-level task graph across turns" \
  --recipe plan \
  --execute-plan \
  --max-iterations 2 \
  --query "code agent project plan task graph" \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --session target/generated/code_project_resume.session.json \
  > target/generated/code_project_resume.first.output.json
cargo run -q -p air-cli -- code "continue executing the same project-level task graph" \
  --recipe plan \
  --execute-plan \
  --max-iterations 2 \
  --query "code agent project plan task graph" \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --session target/generated/code_project_resume.session.json \
  > target/generated/code_project_resume.second.output.json
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_project_resume.first.output.json", encoding="utf-8") as handle:
    first = json.load(handle)
assert first["project"]["status"] == "max_tasks_exhausted", first["project"]
assert first["project"]["executed_tasks"] == 2, first["project"]
assert first["project"]["executed_this_run"] == 2, first["project"]
assert first["project"]["budget"]["task_count"] == len(first["project"]["scheduled_task_ids"]), first["project"]

with open("target/generated/code_project_resume.second.output.json", encoding="utf-8") as handle:
    second = json.load(handle)
project = second["project"]
assert project["status"] == "completed", project
assert project["completed"] is True, project
assert project["resumed_from_turn"] == "turn-000001", project
assert project["executed_tasks"] == 4, project
assert project["executed_this_run"] == 2, project
assert [item["task_id"] for item in project["executions"]] == ["t1", "t2", "t3", "t4"], project
assert project["remaining_task_ids"] == [], project
assert project["budget"]["task_count"] == len(project["scheduled_task_ids"]), project
assert project["budget"]["max_estimated_model_calls"] > 0, project
trace_files = [Path(path) for path in second["trace_files"]]
assert trace_files, second
assert all(".plan." not in path.name for path in trace_files), trace_files
assert all(path.exists() for path in trace_files), trace_files

with open("target/generated/code_project_resume.session.json", encoding="utf-8") as handle:
    session = json.load(handle)
assert len(session["turns"]) == 2, session
assert session["turns"][0]["outputs"]["project"]["status"] == "max_tasks_exhausted", session
assert session["turns"][1]["outputs"]["project"]["status"] == "completed", session
assert session["turns"][1]["outputs"]["project"]["resumed_from_turn"] == "turn-000001", session
PY

echo "[code-agent] project execution respects task dependencies"
cat > target/generated/code_project_dependency_model_fixtures.json <<'JSON'
{
  "fixtures": {
    "project_planner": {
      "summary": "Fixture project plan intentionally lists dependent tasks out of order.",
      "scope": {
        "goal": "Exercise AIR code project dependency scheduling.",
        "non_goals": [],
        "assumptions": ["The executor should run dependencies before dependent tasks."]
      },
      "milestones": [
        {
          "id": "m1",
          "title": "Dependency scheduling",
          "objective": "Run t1 before t2 even though t2 is listed first.",
          "task_ids": ["t1", "t2"]
        }
      ],
      "tasks": [
        {
          "id": "t2",
          "title": "Run after dependency",
          "description": "This task is listed first but depends on t1.",
          "kind": "test",
          "recipe": "explore",
          "input": {
            "task": "Inspect dependency scheduling after the prerequisite task.",
            "query": "code project dependency scheduling",
            "target_path": "crates/air-cli/src/code_agent.rs"
          },
          "depends_on": ["t1"],
          "files": ["crates/air-cli/src/code_agent.rs"],
          "acceptance": []
        },
        {
          "id": "t1",
          "title": "Run dependency first",
          "description": "This prerequisite task is listed second and should run first.",
          "kind": "test",
          "recipe": "explore",
          "input": {
            "task": "Inspect dependency scheduling prerequisite context.",
            "query": "code project dependency scheduling",
            "target_path": "crates/air-cli/src/code_agent.rs"
          },
          "depends_on": [],
          "files": ["crates/air-cli/src/code_agent.rs"],
          "acceptance": []
        }
      ],
      "files": [
        {
          "path": "crates/air-cli/src/code_agent.rs",
          "purpose": "Dependency scheduler implementation.",
          "change_type": "modify"
        }
      ],
      "acceptance": [],
      "risks": [],
      "source_ids": ["docs-code-agent-loop"],
      "next_steps": ["Confirm execution order follows dependencies."]
    },
    "code_explorer": {
      "summary": "Fixture exploration completed for dependency scheduling.",
      "relevant_files": [
        {
          "path": "crates/air-cli/src/code_agent.rs",
          "reason": "Contains project task scheduling."
        }
      ],
      "findings": [
        {
          "title": "Project tasks are dependency ordered",
          "evidence": "The executor should use depends_on instead of raw array order.",
          "source_ids": ["docs-provenance"]
        }
      ],
      "source_ids": ["docs-provenance"],
      "next_steps": ["Run dependent tasks only after prerequisites complete."]
    }
  }
}
JSON
cargo run -q -p air-cli -- code "plan and execute an out-of-order dependency graph" \
  --recipe plan \
  --execute-plan \
  --max-iterations 2 \
  --query "code agent project dependency scheduling" \
  --model-config target/generated/code_project_dependency_model_fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --trace-out target/generated/code_project_dependency.trace.jsonl \
  > target/generated/code_project_dependency.output.json
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_project_dependency.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
project = output["project"]
assert project["status"] == "completed", project
assert project["completed"] is True, project
assert project["scheduled_task_ids"] == ["t1", "t2"], project
assert [item["task_id"] for item in project["executions"]] == ["t1", "t2"], project
assert project["executions"][1]["depends_on"] == ["t1"], project
assert project["remaining_task_ids"] == [], project
assert [item["task_id"] for item in project["budget"]["tasks"]] == ["t1", "t2"], project
assert project["executions"][1]["budget"]["depends_on"] == ["t1"], project
trace_files = [Path(path) for path in output["trace_files"]]
assert len(trace_files) == 3, trace_files
assert all(path.exists() for path in trace_files), trace_files
PY

echo "[code-agent] project execution failure recovery fork"
cat > target/generated/code_project_fail_model_fixtures.json <<'JSON'
{
  "fixtures": {
    "project_planner": {
      "summary": "Fixture project plan intentionally contains a failing acceptance command.",
      "scope": {
        "goal": "Exercise AIR code project recovery when a planned task fails acceptance.",
        "non_goals": [],
        "assumptions": ["The missing acceptance alias should fail without arbitrary shell access."]
      },
      "milestones": [
        {
          "id": "m1",
          "title": "Failure recovery",
          "objective": "Stop project execution and write a recovery fork.",
          "task_ids": ["t1"]
        }
      ],
      "tasks": [
        {
          "id": "t1",
          "title": "Explore before failing acceptance",
          "description": "Run a normal read-only exploration and then fail the declared acceptance gate.",
          "kind": "test",
          "recipe": "explore",
          "input": {
            "task": "Inspect code agent recovery behavior.",
            "query": "code project recovery",
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
          "purpose": "Recovery fork implementation.",
          "change_type": "modify"
        }
      ],
      "acceptance": [],
      "risks": [],
      "source_ids": ["docs-code-agent-loop"],
      "next_steps": ["Inspect the recovery fork."]
    },
    "code_explorer": {
      "summary": "Fixture exploration completed before acceptance failure.",
      "relevant_files": [
        {
          "path": "crates/air-cli/src/code_agent.rs",
          "reason": "Contains code session recovery behavior."
        }
      ],
      "findings": [
        {
          "title": "Recovery is session-scoped",
          "evidence": "The failed project turn should include recovery metadata.",
          "source_ids": ["docs-provenance"]
        }
      ],
      "source_ids": ["docs-provenance"],
      "next_steps": ["Use the failed fork or revert workspace patches."]
    }
  }
}
JSON
rm -f target/generated/code_project_fail.session.json target/generated/code_project_fail.output.json
rm -rf target/generated/code_project_fail.session.traces target/generated/code_project_fail.session.forks
cargo run -q -p air-cli -- code "plan and execute a project that should stop on acceptance failure" \
  --recipe plan \
  --execute-plan \
  --max-iterations 1 \
  --query "code agent project failure recovery" \
  --model-config target/generated/code_project_fail_model_fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --session target/generated/code_project_fail.session.json \
  > target/generated/code_project_fail.output.json
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_project_fail.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
project = output["project"]
assert project["status"] == "stopped", project
assert project["completed"] is False, project
assert project["executions"][0]["task_id"] == "t1", project
assert project["executions"][0]["completed"] is False, project
assert project["executions"][0]["acceptance"][0]["success"] is False, project
assert project["budget"]["task_count"] == 1, project
assert project["executions"][0]["budget"]["task_id"] == "t1", project
recovery = output["recovery"]
assert recovery["status"] == "stopped", recovery
assert recovery["failed_task_id"] == "t1", recovery
assert recovery["failed_execution"]["acceptance"][0]["id"] == "missing-alias", recovery
assert recovery["failed_execution"]["output_keys"] == ["exploration"], recovery
fork = Path(recovery["fork"])
assert fork.exists(), recovery
assert recovery["workspace_revert_argv"][:3] == ["air", "code-session", recovery["fork"]], recovery
project_artifacts = output["project"]["artifacts"]
assert any(item["id"] == "recovery-fork:turn-000001" for item in project_artifacts), project_artifacts

with open("target/generated/code_project_fail.session.json", encoding="utf-8") as handle:
    session = json.load(handle)
turn = session["turns"][-1]
assert turn["completed"] is False, turn
assert turn["recovery"]["fork"] == recovery["fork"], turn
assert turn["outputs"]["recovery"]["fork"] == recovery["fork"], turn
with fork.open(encoding="utf-8") as handle:
    fork_session = json.load(handle)
assert fork_session["turns"][-1]["id"] == turn["id"], fork_session
assert fork_session["turns"][-1]["recovery"]["failed_task_id"] == "t1", fork_session
PY

cargo run -q -p air-cli -- code "continue from the failed project execution" \
  --recipe plan \
  --execute-plan \
  --max-iterations 1 \
  --query "code agent project failure recovery" \
  --model-config target/generated/code_project_fail_model_fixtures.json \
  --tool-config examples/code-agent/tools.json \
  --session target/generated/code_project_fail.session.json \
  --explain \
  > target/generated/code_project_fail_recovery_explain.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_project_fail_recovery_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
task = output["input"]["task"]
assert "AIR project recovery context from previous failed execution" in task, task
assert '"failed_task_id": "t1"' in task, task
assert "missing-alias" in task, task
assert "target/generated/code_project_fail.session.forks/turn1.failed.json" in task, task
PY

cat > target/generated/code_project_recover_model_fixtures.json <<'JSON'
{
  "fixtures": {
    "project_planner": {
      "summary": "Fixture recovery project plan: use the prior failed execution context to run a bounded repair task.",
      "scope": {
        "goal": "Recover from the stopped project execution by repairing the failing fixture and verifying it.",
        "non_goals": [],
        "assumptions": ["The previous stopped turn identified t1 and its failing acceptance context."]
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
      "next_steps": ["Verify the repair output and continue remaining project tasks if any."]
    },
    "code_explorer": {
      "summary": "Fixture exploration completed for recovery repair.",
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
          "evidence": "The repair fixture test expects add(2, 3) to equal 5.",
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
echo "[code-agent] project recovery replans and executes repair"
recovery_repair_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$recovery_repair_backup"
restore_recovery_repair_fixture() {
  cp "$recovery_repair_backup" examples/code-agent/repair-fixture/math.js
  rm -f "$recovery_repair_backup"
}
trap restore_recovery_repair_fixture EXIT
cargo run -q -p air-cli -- code "recover and repair the failed project execution" \
  --recipe plan \
  --execute-plan \
  --max-iterations 1 \
  --query "code agent project failure recovery" \
  --model-config target/generated/code_project_recover_model_fixtures.json \
  --tool-config examples/code-agent/tools.core.json \
  --session target/generated/code_project_fail.session.json \
  > target/generated/code_project_recover.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code_project_recover.post_test.log
restore_recovery_repair_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_project_recover.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
project = output["project"]
assert project["status"] == "completed", project
assert project["completed"] is True, project
assert project["executed_this_run"] == 1, project
assert project["resumed_from_turn"] is None, project
assert [item["task_id"] for item in project["executions"]] == ["fix-t1"], project
repair = project["executions"][0]["outputs"]["repair"]
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert "examples/code-agent/repair-fixture/math.js" in repair["workspace_diff"]["diff"], repair

with open("target/generated/code_project_fail.session.json", encoding="utf-8") as handle:
    session = json.load(handle)
assert len(session["turns"]) == 2, session
assert session["turns"][0]["outputs"]["project"]["status"] == "stopped", session
assert session["turns"][1]["outputs"]["project"]["status"] == "completed", session
task = session["turns"][1]["input"]["task"]
assert "AIR project recovery context from previous failed execution" in task, task
PY

echo "[code-agent] user-facing code command explain"
cargo run -q -p air-cli -- code "fix the failing add function and retest" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --explain \
  > target/generated/code_command_explain.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
assert output["command"] == "code", output
assert output["will_run"] is False, output
assert output["requested_recipe"] == "auto", output
assert output["resolved_recipe"] == "repair", output
assert output["pack"]["path"] == "examples/code-agent/code-agent.air-pack.yaml", output
assert output["pack"]["recipe"] == "repair", output
assert output["pack"]["default_profile"] == "examples/code-agent/repair-core.air-profile.yaml", output
assert output["pack"]["profile_override"] is False, output
assert output["profile"] == "examples/code-agent/repair-core.air-profile.yaml", output
assert output["plan"].endswith("examples/code-agent/code-repair.air-plan.yaml"), output
assert output["store"].endswith("examples/code-agent/module-store.air-store.yaml"), output
assert "file.write" in output["capabilities"], output
assert output["read_only"] is False, output
assert output["writes_workspace"] is True, output
assert output["budget"]["per_iteration"]["max_estimated_model_calls"] > 0, output
assert output["budget"]["per_iteration"]["max_estimated_tool_calls"] > 0, output
assert output["budget"]["total"]["iterations"] == 1, output
assert output["budget"]["total"]["max_estimated_tool_calls"] == output["budget"]["per_iteration"]["max_estimated_tool_calls"], output
assert output["budget_limit"]["exceeded"] is False, output
assert output["loop"]["enabled"] is False, output
assert output["loop"]["max_iterations"] == 3, output
assert output["input"]["test_command"] == "repair_fixture_test", output
PY

cargo run -q -p air-cli -- code "fix the failing add function through explicit composed repair profile" \
  --recipe repair \
  --profile examples/code-agent/repair.air-profile.yaml \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --explain \
  > target/generated/code_command_profile_override_explain.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_profile_override_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
assert output["resolved_recipe"] == "repair", output
assert output["profile"] == "examples/code-agent/repair.air-profile.yaml", output
assert output["pack"]["recipe"] == "repair", output
assert output["pack"]["default_profile"] == "examples/code-agent/repair-core.air-profile.yaml", output
assert output["pack"]["profile_override"] is True, output
PY

cargo run -q -p air-cli -- code "fix the failing add function until tests pass" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --loop \
  --max-iterations 2 \
  --explain \
  > target/generated/code_command_loop_explain.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_loop_explain.output.json", encoding="utf-8") as handle:
    output = json.load(handle)

per = output["budget"]["per_iteration"]
total = output["budget"]["total"]
assert output["loop"]["enabled"] is True, output
assert total["iterations"] == 2, output
assert total["max_estimated_model_calls"] == per["max_estimated_model_calls"] * 2, output
assert total["max_estimated_tool_calls"] == per["max_estimated_tool_calls"] * 2, output
PY

check_code_agent_route() {
  local name="$1"
  local task="$2"
  local expected_id="$3"
  local expected_source="$4"
  local output="target/generated/code_agent_plan_${name}.json"

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
assert first["tier"] == "large_component"
for candidate in output["component_selection"]["recommended"]:
    assert candidate["matched_terms"], candidate
PY
}

echo "[code-agent] primary component routing explain"
check_code_agent_route \
  "plan" \
  "Plan a project-level task graph with milestones, acceptance criteria, and source-grounded file touchpoints." \
  "code.project_plan_recipe@0.1.0" \
  "recipe"
check_code_agent_route \
  "review" \
  "Review the command_run implementation for safety, provenance, and diagnostics." \
  "code.review_with_std_context@0.1.0" \
  "recipe"
check_code_agent_route \
  "repair" \
  "Fix a failing test using structured diagnostics, apply bounded file operations, and retest." \
  "code.core_repair@0.1.0" \
  "recipe"
check_code_agent_route \
  "refactor" \
  "Refactor a passing implementation while preserving behavior with an allowlisted test." \
  "code.core_refactor@0.1.0" \
  "recipe"
check_code_agent_route \
  "open_refactor" \
  "Refactor the codebase to make a simple implementation cleaner when no target file is supplied." \
  "code.open_refactor@0.1.0" \
  "recipe"
check_code_agent_route \
  "build" \
  "Build an Apple-style landing page as a single HTML file and verify screenshots." \
  "code.build_page@0.1.0" \
  "module"
check_code_agent_route \
  "explore" \
  "Explore how command_run is implemented and identify relevant repository files." \
  "code.explore@0.1.0" \
  "module"
check_code_agent_route \
  "dynamic_explore" \
  "Explore the repository with a dynamic plan-act-observe loop that lets the model choose declared read-only tools." \
  "code.dynamic_explore@0.1.0" \
  "module"

echo "[code-agent] read-only explore offline run"
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/explore.air-profile.yaml \
  > target/generated/code_explore_fixture.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_explore_fixture.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
exploration = output["exploration"]
assert exploration["summary"].startswith("Fixture exploration completed")
assert any(item["path"] == "crates/air-tools/src/lib.rs" for item in exploration["relevant_files"])
assert exploration["findings"][0]["source_ids"]
PY

echo "[code-agent] dynamic read-only explore offline run"
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/dynamic-explore.air-profile.yaml \
  --trace-out target/generated/code_dynamic_explore_fixture.trace.jsonl \
  > target/generated/code_dynamic_explore_fixture.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_dynamic_explore_fixture.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
exploration = output["exploration"]
assert exploration["summary"].startswith("Fixture dynamic exploration completed")
assert any(item["path"] == "crates/air-runtime/src/lib.rs" for item in exploration["relevant_files"])

with open("target/generated/code_dynamic_explore_fixture.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]
dispatches = [
    event for event in events
    if event.get("action") == "tool_batch_dispatch_item"
    and event.get("meta", {}).get("tool") in {
        "repo.files",
        "repo.search",
        "repo.symbols",
        "repo.references",
        "repo.context",
        "file.read_many",
    }
]
seen_tools = {event.get("meta", {}).get("tool") for event in dispatches}
assert {
    "repo.files",
    "repo.search",
    "repo.symbols",
    "repo.references",
    "repo.context",
    "file.read_many",
} <= seen_tools, seen_tools
batch_done = [
    event for event in events
    if event.get("action") == "tool_batch_dispatch"
]
batch_counts = [event.get("meta", {}).get("count") for event in batch_done]
assert batch_counts == [4, 2], batch_counts
PY

echo "[code-agent] user-facing explore command offline run"
cargo run -q -p air-cli -- code "explore command_run safety" \
  --target crates/air-tools/src/lib.rs \
  --query command_run \
  > target/generated/code_command_explore.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_command_explore.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
exploration = output["exploration"]
assert exploration["summary"].startswith("Fixture exploration completed")
assert any(item["path"] == "crates/air-tools/src/lib.rs" for item in exploration["relevant_files"])
assert exploration["next_steps"], exploration
PY

echo "[code-agent] persistent session carries turn context"
rm -f target/generated/code_agent_session.json
rm -rf target/generated/code_agent_session.traces
cargo run -q -p air-cli -- code "explore command_run safety" \
  --target crates/air-tools/src/lib.rs \
  --query command_run \
  --session target/generated/code_agent_session.json \
  > target/generated/code_agent_session_turn1.output.json
cargo run -q -p air-cli -- code "continue from the previous AIR code-agent turn" \
  --target crates/air-tools/src/lib.rs \
  --query command_run \
  --session target/generated/code_agent_session.json \
  > target/generated/code_agent_session_turn2.output.json
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_agent_session.json", encoding="utf-8") as handle:
    session = json.load(handle)
assert session["version"] == 1, session
assert len(session["turns"]) == 2, session
assert session["turns"][0]["recipe"] == "explore", session
assert session["turns"][1]["completed"] is True, session
for index, turn in enumerate(session["turns"], start=1):
    assert turn["id"] == f"turn-{index:06}", turn
    assert turn["time"]["created"] > 0, turn
    assert turn["time"]["updated"] >= turn["time"]["created"], turn
    assert turn["pack"]["path"] == "examples/code-agent/code-agent.air-pack.yaml", turn
    assert turn["pack"]["recipe"] == "explore", turn
    assert turn["pack"]["default_profile"] == "examples/code-agent/explore.air-profile.yaml", turn
    assert turn["pack"]["profile_override"] is False, turn
    assert turn["trace_files"], turn
    assert Path(turn["trace_files"][0]).exists(), turn
    assert turn["trace_files"][0].endswith(f"turn{index}.trace.jsonl"), turn
    assert turn["summary"]["model_call_count"] >= 1, turn
    assert turn["summary"]["tool_call_count"] >= 1, turn
    assert any(part["kind"] == "model_call" for part in turn["parts"]), turn
    assert any(part["kind"] == "tool_call" for part in turn["parts"]), turn

with open(session["turns"][1]["trace_files"][0], encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]
model_calls = [
    event for event in trace
    if event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_explorer"
]
assert model_calls, trace
task = model_calls[0]["input"]["task"]
assert "AIR session context from previous turns" in task, task
assert "Fixture exploration completed" in task, task
assert "models=code_explorer" in task, task
assert "tools=" in task, task
assert any(
    part["kind"] == "model_call" and part.get("model") == "code_explorer"
    for part in session["turns"][1]["parts"]
), session["turns"][1]["parts"]
PY

echo "[code-agent] session fork and revert"
cargo run -q -p air-cli -- code-session target/generated/code_agent_session.json \
  --fork target/generated/code_agent_session_fork.json \
  --revert-to turn-000001 \
  > target/generated/code_agent_session_fork.output.json
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_session_fork.output.json", encoding="utf-8") as handle:
    summary = json.load(handle)
assert summary["source"] == "target/generated/code_agent_session.json", summary
assert summary["output"] == "target/generated/code_agent_session_fork.json", summary
assert summary["original_turn_count"] == 2, summary
assert summary["turn_count"] == 1, summary
assert summary["reverted_to"] == "turn-000001", summary
assert summary["workspace_reverted"] is False, summary

with open("target/generated/code_agent_session.json", encoding="utf-8") as handle:
    original = json.load(handle)
with open("target/generated/code_agent_session_fork.json", encoding="utf-8") as handle:
    fork = json.load(handle)
assert len(original["turns"]) == 2, original
assert len(fork["turns"]) == 1, fork
assert fork["turns"][0]["id"] == "turn-000001", fork
assert fork["turns"][0]["trace_files"], fork
PY

echo "[code-agent] repair fixture starts failing with a structured diagnostic"
if node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_repair_fixture.log 2>&1; then
  echo "repair fixture unexpectedly passed; it should start from a failing implementation" >&2
  cat target/generated/code_agent_repair_fixture.log >&2
  exit 1
fi
grep -q "examples/code-agent/repair-fixture/math.js:2:10: error:" \
  target/generated/code_agent_repair_fixture.log

echo "[code-agent] multifile repair fixture starts failing with structured diagnostics"
if node examples/code-agent/repair-multifile/test.js > target/generated/code_agent_repair_multifile_fixture.log 2>&1; then
  echo "multifile repair fixture unexpectedly passed; it should start from a failing implementation" >&2
  cat target/generated/code_agent_repair_multifile_fixture.log >&2
  exit 1
fi
grep -q "examples/code-agent/repair-multifile/math.js:4:10: error:" \
  target/generated/code_agent_repair_multifile_fixture.log

echo "[code-agent] refactor fixture starts passing before forced patch"
node examples/code-agent/refactor-fixture/test.js > target/generated/code_agent_refactor_fixture.log

echo "[code-agent] core explore-repair offline run"
repair_fixture_backup="$(mktemp)"
repair_fixture_user_dirty_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$repair_fixture_backup"
cp examples/code-agent/repair-fixture/test.js "$repair_fixture_user_dirty_backup"
restore_core_repair_fixture() {
  cp "$repair_fixture_backup" examples/code-agent/repair-fixture/math.js
  cp "$repair_fixture_user_dirty_backup" examples/code-agent/repair-fixture/test.js
  rm -f "$repair_fixture_backup" "$repair_fixture_user_dirty_backup"
}
trap restore_core_repair_fixture EXIT
printf "\n// pre-existing user dirty change that repair diff must not include\n" >> examples/code-agent/repair-fixture/test.js
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/repair-core.air-profile.yaml \
  --trace-out target/generated/code_agent_repair_core.trace.jsonl \
  > target/generated/code_agent_repair_core.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_repair_core.post_test.log
restore_core_repair_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_repair_core.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_repair_core.trace.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

repair = output["repair"]
assert repair["initial_success"] is False, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert repair["changed_files"], repair
assert repair["workspace_changed_files"], repair
assert repair["workspace_clean_before"] is False, repair
preexisting = {entry["path"] for entry in repair["preexisting_changed_files"]}
assert "examples/code-agent/repair-fixture/test.js" in preexisting, repair
assert "examples/code-agent/repair-fixture/math.js" in repair["workspace_diff"]["diff"], repair
assert "examples/code-agent/repair-fixture/test.js" not in repair["workspace_diff"]["diff"], repair
assert not any(
    event.get("action") == "model_call"
    and event.get("meta", {}).get("model") in {"code_explorer", "code_repair_context_selector"}
    for event in trace
), trace
assert any(
    event.get("action") == "approval"
    and event.get("status") == "ok"
    and "file.write" in event.get("meta", {}).get("approval_for", [])
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.ops"
    and event.get("status") == "ok"
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "todo.write"
    and event.get("status") == "ok"
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "todo.read"
    and event.get("status") == "ok"
    for event in trace
), trace
repair_calls = [
    event for event in trace
    if event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_repairer"
]
assert repair_calls, trace
assert repair_calls[0]["input"]["progress"]["in_progress_count"] == 1, repair_calls[0]["input"]
assert any(
    todo["id"] == "repair" and todo["status"] == "in_progress"
    for todo in repair_calls[0]["input"]["progress"]["todos"]
), repair_calls[0]["input"]
PY

echo "[code-agent] core refactor offline run"
refactor_fixture_backup="$(mktemp)"
cp examples/code-agent/refactor-fixture/math.js "$refactor_fixture_backup"
restore_refactor_fixture() {
  cp "$refactor_fixture_backup" examples/code-agent/refactor-fixture/math.js
  rm -f "$refactor_fixture_backup"
}
trap restore_refactor_fixture EXIT
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/refactor-core.air-profile.yaml \
  --trace-out target/generated/code_agent_refactor_core.trace.jsonl \
  > target/generated/code_agent_refactor_core.output.json
node examples/code-agent/refactor-fixture/test.js > target/generated/code_agent_refactor_core.post_test.log
restore_refactor_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_refactor_core.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_refactor_core.trace.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

repair = output["repair"]
refactor = output["refactor"]
assert repair == refactor, output
assert repair["initial_success"] is True, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert repair["changed_files"], repair
assert any(
    entry["path"] == "examples/code-agent/refactor-fixture/math.js"
    for entry in repair["changed_files"]
), repair
assert "reduce" in repair["rationale"], repair
assert any(
    event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_repairer"
    and event.get("status") == "ok"
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "test.run"
    and event.get("status") == "ok"
    and event.get("output", {}).get("success") is True
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.ops"
    and event.get("status") == "ok"
    for event in trace
), trace
PY

echo "[code-agent] open-ended refactor offline run"
open_refactor_fixture_backup="$(mktemp)"
cp examples/code-agent/refactor-fixture/math.js "$open_refactor_fixture_backup"
restore_open_refactor_fixture() {
  cp "$open_refactor_fixture_backup" examples/code-agent/refactor-fixture/math.js
  rm -f "$open_refactor_fixture_backup"
}
trap restore_open_refactor_fixture EXIT
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/open-refactor.air-profile.yaml \
  --trace-out target/generated/code_agent_open_refactor.trace.jsonl \
  > target/generated/code_agent_open_refactor.output.json
node examples/code-agent/refactor-fixture/test.js > target/generated/code_agent_open_refactor.post_test.log
restore_open_refactor_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_open_refactor.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_open_refactor.trace.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

candidate = output["refactor_candidate"]
assert candidate["target_path"] == "examples/code-agent/refactor-fixture/math.js", candidate
assert candidate["test_command"] == "refactor_fixture_test", candidate
assert candidate["risk"] == "low", candidate
repair = output["repair"]
assert output["refactor"] == repair, output
assert repair["initial_success"] is True, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
models = [
    event.get("meta", {}).get("model")
    for event in trace
    if event.get("action") == "model_call" and event.get("status") == "ok"
]
assert models == ["code_explorer", "code_refactor_candidate_selector", "code_repairer"], models
PY

echo "[code-agent] user-facing refactor command offline run"
refactor_command_backup="$(mktemp)"
cp examples/code-agent/refactor-fixture/math.js "$refactor_command_backup"
restore_refactor_command_fixture() {
  cp "$refactor_command_backup" examples/code-agent/refactor-fixture/math.js
  rm -f "$refactor_command_backup"
}
trap restore_refactor_command_fixture EXIT
cargo run -q -p air-cli -- code "refactor the sum implementation while keeping tests passing" \
  --recipe refactor \
  --target examples/code-agent/refactor-fixture/math.js \
  --test refactor_fixture_test \
  --related examples/code-agent/refactor-fixture/test.js \
  --model-config examples/code-agent/model-fixtures.refactor.json \
  --tool-config examples/code-agent/tools.core.json \
  > target/generated/code_agent_refactor_command.output.json
node examples/code-agent/refactor-fixture/test.js > target/generated/code_agent_refactor_command.post_test.log
restore_refactor_command_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_refactor_command.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
repair = output["repair"]
assert output["refactor"] == repair, output
assert repair["initial_success"] is True, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert any(
    entry["path"] == "examples/code-agent/refactor-fixture/math.js"
    for entry in repair["changed_files"]
), repair
PY

echo "[code-agent] user-facing code command offline run"
code_command_fixture_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$code_command_fixture_backup"
restore_code_command_fixture() {
  cp "$code_command_fixture_backup" examples/code-agent/repair-fixture/math.js
  rm -f "$code_command_fixture_backup"
}
trap restore_code_command_fixture EXIT
rm -f target/generated/code_agent_code_command.session.json
rm -rf target/generated/code_agent_code_command.session.traces
cargo run -q -p air-cli -- code "fix the failing add function and retest" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --related examples/code-agent/repair-fixture/test.js \
  --session target/generated/code_agent_code_command.session.json \
  > target/generated/code_agent_code_command.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_code_command.post_test.log
cargo run -q -p air-cli -- code-session target/generated/code_agent_code_command.session.json \
  --revert-workspace-turn turn-000001 \
  > target/generated/code_agent_code_command.revert_check.json
cargo run -q -p air-cli -- code-session target/generated/code_agent_code_command.session.json \
  --revert-workspace-turn turn-000001 \
  --apply-workspace \
  > target/generated/code_agent_code_command.revert_apply.json
if node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_code_command.reverted_test.log 2>&1; then
  echo "repair fixture unexpectedly passed after reverse patch apply" >&2
  cat target/generated/code_agent_code_command.reverted_test.log >&2
  exit 1
fi
restore_code_command_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_agent_code_command.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_code_command.session.json", encoding="utf-8") as handle:
    session = json.load(handle)
with open("target/generated/code_agent_code_command.revert_check.json", encoding="utf-8") as handle:
    revert_check = json.load(handle)
with open("target/generated/code_agent_code_command.revert_apply.json", encoding="utf-8") as handle:
    revert_apply = json.load(handle)
assert len(session["turns"]) == 1, session
turn = session["turns"][0]
assert turn["trace_files"], turn
assert Path(turn["trace_files"][0]).exists(), turn
assert any(
    part["kind"] == "tool_call"
    and part.get("tool") == "file.ops"
    and "examples/code-agent/repair-fixture/math.js" in part.get("files", [])
    and "file_ops" in part.get("artifact_kinds", [])
    for part in turn["parts"]
), turn["parts"]
assert "file.ops" in turn["summary"]["tools"], turn["summary"]
assert "file.write" in turn["summary"]["approvals"], turn["summary"]
assert "examples/code-agent/repair-fixture/math.js" in turn["summary"]["files"], turn["summary"]
assert "file_ops" in turn["summary"]["artifact_kinds"], turn["summary"]
assert turn["patch_sets"], turn
patch_set = turn["patch_sets"][0]
assert patch_set["source"].endswith(".repair"), patch_set
assert patch_set["workspace_clean_before"] is False, patch_set
assert any(
    entry["path"] == "examples/code-agent/repair-fixture/math.js"
    for entry in patch_set["changed_files"]
), patch_set
assert "examples/code-agent/repair-fixture/math.js" in patch_set["diff"], patch_set
assert revert_check["workspace_reverted"] is False, revert_check
workspace_revert = revert_check["workspace_revert"]
assert workspace_revert["turn_id"] == "turn-000001", workspace_revert
assert workspace_revert["success"] is True, workspace_revert
assert workspace_revert["applied"] is False, workspace_revert
assert workspace_revert["results"], workspace_revert
assert workspace_revert["results"][0]["checked"] is True, workspace_revert
assert workspace_revert["results"][0]["success"] is True, workspace_revert
assert revert_apply["workspace_reverted"] is True, revert_apply
workspace_apply = revert_apply["workspace_revert"]
assert workspace_apply["turn_id"] == "turn-000001", workspace_apply
assert workspace_apply["success"] is True, workspace_apply
assert workspace_apply["applied"] is True, workspace_apply
assert len(workspace_apply["results"]) == 2, workspace_apply
assert workspace_apply["results"][0]["checked"] is True, workspace_apply
assert workspace_apply["results"][1]["applied"] is True, workspace_apply
assert workspace_apply["results"][1]["success"] is True, workspace_apply
with open(turn["trace_files"][0], encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

repair = output["repair"]
changed = {entry["path"] for entry in repair["changed_files"]}
assert repair["initial_success"] is False, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert "examples/code-agent/repair-fixture/math.js" in changed, repair
assert "examples/code-agent/repair-fixture/math.js" in repair["workspace_diff"]["diff"], repair
assert not any(
    event.get("action") == "model_call"
    and event.get("meta", {}).get("model") in {"code_explorer", "code_repair_context_selector"}
    for event in trace
), trace
assert any(
    event.get("action") == "approval"
    and event.get("status") == "ok"
    and "file.write" in event.get("meta", {}).get("approval_for", [])
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.ops"
    and event.get("status") == "ok"
    for event in trace
), trace
PY

echo "[code-agent] bounded loop repair command offline run"
code_loop_fixture_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$code_loop_fixture_backup"
restore_code_loop_fixture() {
  cp "$code_loop_fixture_backup" examples/code-agent/repair-fixture/math.js
  rm -f "$code_loop_fixture_backup"
}
trap restore_code_loop_fixture EXIT
cargo run -q -p air-cli -- code "fix the failing add function until tests pass" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --related examples/code-agent/repair-fixture/test.js \
  --loop \
  --max-iterations 2 \
  --model-config examples/code-agent/model-fixtures.json \
  --tool-config examples/code-agent/tools.core.json \
  --trace-out target/generated/code_agent_code_loop.trace.jsonl \
  > target/generated/code_agent_code_loop.output.json
node examples/code-agent/repair-fixture/test.js > target/generated/code_agent_code_loop.post_test.log
restore_code_loop_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_agent_code_loop.output.json", encoding="utf-8") as handle:
    output = json.load(handle)

assert output["status"] == "completed", output
assert output["completed"] is True, output
assert output["recipe"] == "repair", output
assert len(output["iterations"]) == 1, output
repair = output["final_outputs"]["repair"]
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert "examples/code-agent/repair-fixture/math.js" in repair["workspace_diff"]["diff"], repair
assert Path("target/generated/code_agent_code_loop.trace.iter1.jsonl").exists()
PY

echo "[code-agent] bounded loop carries failed iteration context"
code_loop_feedback_backup="$(mktemp)"
cp examples/code-agent/repair-fixture/math.js "$code_loop_feedback_backup"
restore_code_loop_feedback_fixture() {
  cp "$code_loop_feedback_backup" examples/code-agent/repair-fixture/math.js
  rm -f "$code_loop_feedback_backup"
}
trap restore_code_loop_feedback_fixture EXIT
rm -f target/generated/code_agent_code_loop_feedback.session.json
rm -rf target/generated/code_agent_code_loop_feedback.session.traces
cargo run -q -p air-cli -- code "fix the failing add function until tests pass" \
  --target examples/code-agent/repair-fixture/math.js \
  --test repair_fixture_test \
  --related examples/code-agent/repair-fixture/test.js \
  --loop \
  --max-iterations 2 \
  --model-config examples/code-agent/model-fixtures.loop-fail.json \
  --tool-config examples/code-agent/tools.core.json \
  --session target/generated/code_agent_code_loop_feedback.session.json \
  > target/generated/code_agent_code_loop_feedback.output.json
restore_code_loop_feedback_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json
from pathlib import Path

with open("target/generated/code_agent_code_loop_feedback.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_code_loop_feedback.session.json", encoding="utf-8") as handle:
    session = json.load(handle)

assert output["status"] == "max_iterations_exhausted", output
assert output["completed"] is False, output
assert len(output["iterations"]) == 2, output
assert output["iterations"][0]["outputs"]["repair"]["final_success"] is False, output
assert len(session["turns"]) == 1, session
turn = session["turns"][0]
assert turn["id"] == "turn-000001", turn
assert turn["completed"] is False, turn
assert len(turn["trace_files"]) == 2, turn
assert all(Path(path).exists() for path in turn["trace_files"]), turn
assert turn["trace_files"][0].endswith("turn1.trace.iter1.jsonl"), turn
assert turn["trace_files"][1].endswith("turn1.trace.iter2.jsonl"), turn
assert turn["summary"]["model_call_count"] >= 2, turn
assert turn["summary"]["tool_call_count"] >= 2, turn
assert "file.ops" in turn["summary"]["tools"], turn

with open(turn["trace_files"][1], encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

repair_calls = [
    event for event in trace
    if event.get("action") == "model_call"
    and event.get("meta", {}).get("model") == "code_repairer"
]
assert repair_calls, trace
task = repair_calls[0]["input"]["task"]
assert "AIR loop context from previous iterations" in task, task
assert '"final_success":false' in task, task
assert "file.ops" in task, task
assert "old_string was not found" in task, task
PY

echo "[code-agent] core multifile repair offline run"
multifile_math_backup="$(mktemp)"
multifile_normalize_backup="$(mktemp)"
cp examples/code-agent/repair-multifile/math.js "$multifile_math_backup"
cp examples/code-agent/repair-multifile/normalize.js "$multifile_normalize_backup"
restore_multifile_repair_fixture() {
  cp "$multifile_math_backup" examples/code-agent/repair-multifile/math.js
  cp "$multifile_normalize_backup" examples/code-agent/repair-multifile/normalize.js
  rm -f "$multifile_math_backup" "$multifile_normalize_backup"
}
trap restore_multifile_repair_fixture EXIT
cargo run -q -p air-cli -- run-plan --profile examples/code-agent/repair-multifile.air-profile.yaml \
  --trace-out target/generated/code_agent_repair_multifile.trace.jsonl \
  > target/generated/code_agent_repair_multifile.output.json
node examples/code-agent/repair-multifile/test.js > target/generated/code_agent_repair_multifile.post_test.log
restore_multifile_repair_fixture
trap - EXIT
"${PYTHON:-python3}" - <<'PY'
import json

with open("target/generated/code_agent_repair_multifile.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
with open("target/generated/code_agent_repair_multifile.trace.jsonl", encoding="utf-8") as handle:
    trace = [json.loads(line) for line in handle if line.strip()]

repair = output["repair"]
changed = {entry["path"] for entry in repair["changed_files"]}
workspace_changed = {entry["path"] for entry in repair["workspace_changed_files"]}
assert output["repair_context"]["related_files"], output
assert repair["initial_success"] is False, repair
assert repair["final_success"] is True, repair
assert repair["patch_applied"] is True, repair
assert "examples/code-agent/repair-multifile/math.js" in changed, repair
assert "examples/code-agent/repair-multifile/normalize.js" in changed, repair
assert changed <= workspace_changed, repair
assert "examples/code-agent/repair-multifile/math.js" in repair["workspace_diff"]["diff"], repair
assert "examples/code-agent/repair-multifile/normalize.js" in repair["workspace_diff"]["diff"], repair
assert any(
    event.get("action") == "approval"
    and event.get("status") == "ok"
    and "file.write" in event.get("meta", {}).get("approval_for", [])
    for event in trace
), trace
assert any(
    event.get("action") == "tool_call"
    and event.get("meta", {}).get("tool") == "file.ops"
    and event.get("status") == "ok"
    and event.get("output", {}).get("file_count") == 2
    for event in trace
), trace
PY

if [[ "${AIR_CODE_AGENT_REAL:-0}" == "1" ]]; then
  echo "[code-agent] real repair smoke"
  original="$(cat examples/code-agent/repair-fixture/math.js)"
  restore_fixture() {
    printf "%s" "$original" > examples/code-agent/repair-fixture/math.js
  }
  trap restore_fixture EXIT

  cargo run -q -p air-cli -- run-plan --profile examples/code-agent/repair.air-profile.yaml --log \
    > target/generated/code_agent_repair_real.output.json
  node examples/code-agent/repair-fixture/test.js
  restore_fixture
  trap - EXIT
fi

echo "[code-agent] verification complete"
