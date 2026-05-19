#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCENARIO="${AIR_E2E_DREAM_DIR:-/tmp/air-dream-e2e-company-service}"
AIR_BIN="${AIR_BIN:-$ROOT/target/debug/air}"
REAL_PROVIDER="${AIR_E2E_REAL_PROVIDER:-0}"
MODEL_CONFIG="${AIR_E2E_MODEL_CONFIG:-$ROOT/examples/local-openai-compatible.json}"

if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for this E2E smoke test" >&2
  exit 2
fi

cargo build -q -p air-cli

rm -rf "$SCENARIO"
mkdir -p "$SCENARIO/skills" "$SCENARIO/modules" "$SCENARIO/company-service/src" \
  "$SCENARIO/company-service/test" "$SCENARIO/target/generated/code-runs" "$SCENARIO/.air"
cp -R "$ROOT/skills/code-agent" "$SCENARIO/skills/code-agent"
cp -R "$ROOT/modules/std" "$SCENARIO/modules/std"

cat > "$SCENARIO/company-service/src/billing.js" <<'JS'
function addCents(a, b) {
  return a - b;
}

module.exports = { addCents };
JS

cat > "$SCENARIO/company-service/test/billing.test.js" <<'JS'
const assert = require("assert");
const { addCents } = require("../src/billing");

assert.strictEqual(addCents(120, 30), 150);
console.log("billing tests passed");
JS

if [[ "$REAL_PROVIDER" != "1" ]]; then
  cat > "$SCENARIO/.air/e2e-model-fixtures.json" <<'JSON'
{
  "fixtures": {
    "code_edit_decider": {
      "$sequence": [
        {
          "complete": false,
          "tool_calls": [
            {
              "tool": "bash",
              "input": {
                "command": "node company-service/test/billing.test.js",
                "description": "Run the billing test"
              }
            }
          ]
        },
        {
          "complete": false,
          "tool_calls": [
            {
              "tool": "read_range",
              "input": {
                "file": "company-service/src/billing.js",
                "offset": 0,
                "limit": 40
              }
            }
          ]
        },
        {
          "complete": false,
          "tool_calls": [
            {
              "tool": "edit",
              "input": {
                "filePath": "company-service/src/billing.js",
                "oldString": "  return a - b;",
                "newString": "  return a + b;"
              }
            }
          ]
        },
        {
          "complete": false,
          "tool_calls": [
            {
              "tool": "bash",
              "input": {
                "command": "node company-service/test/billing.test.js",
                "description": "Run the billing test after the edit"
              }
            }
          ]
        },
        {
          "complete": true,
          "tool_calls": []
        }
      ]
    }
  }
}
JSON
  MODEL_CONFIG="$SCENARIO/.air/e2e-model-fixtures.json"
else
  : "${AIR_MODEL_LOCAL_API_KEY:?AIR_E2E_REAL_PROVIDER=1 requires AIR_MODEL_LOCAL_API_KEY in the environment or .env}"
  : "${AIR_MODEL_LOCAL_BASE_URL:?AIR_E2E_REAL_PROVIDER=1 requires AIR_MODEL_LOCAL_BASE_URL in the environment or .env}"
  : "${AIR_MODEL_LOCAL_MODEL:?AIR_E2E_REAL_PROVIDER=1 requires AIR_MODEL_LOCAL_MODEL in the environment or .env}"
fi

cd "$SCENARIO"
git init -q
printf '.air/\ntarget/\n' > .gitignore
git add .gitignore company-service skills modules
GIT_AUTHOR_NAME='AIR E2E' GIT_AUTHOR_EMAIL='air-e2e@example.com' \
  GIT_COMMITTER_NAME='AIR E2E' GIT_COMMITTER_EMAIL='air-e2e@example.com' \
  git commit -qm 'seed company service fixture'

set +e
"$AIR_BIN" run --mode code --memory \
  --model-config "$MODEL_CONFIG" \
  --tool-config skills/code-agent/tools.json \
  --artifact-out target/generated/code-runs/day-success \
  "Fix the billing cents addition bug and run the appropriate test." \
  > target/generated/day-success.run.json
RUN_STATUS=$?
set -e

if [[ ! -f target/generated/code-runs/day-success/artifact.json ]]; then
  echo "air run did not produce target/generated/code-runs/day-success/artifact.json; status=$RUN_STATUS" >&2
  echo "run output:" >&2
  sed -n '1,120p' target/generated/day-success.run.json >&2 || true
  exit 1
fi

mkdir -p target/generated/code-runs/day-failure
cat > target/generated/code-runs/day-failure/artifact.json <<'JSON'
{
  "schema": "air.code_run_artifact.v1",
  "descriptor": {
    "task": "Fix the order retry bug and verify it.",
    "cwd": "/tmp/air-dream-e2e-company-service",
    "trace": "target/generated/code-runs/day-failure/trace.jsonl",
    "outputs": "target/generated/code-runs/day-failure/output.json",
    "created_at_unix": 1770000001,
    "constraints": {
      "require_patch": true,
      "require_verification": true
    }
  },
  "verdict": {
    "schema": "air.code_run_verdict.v1",
    "final_success": false,
    "category": "verification_failed",
    "reason": "agent reported progress but no passing verification was present after the patch attempt",
    "patch_applied": true,
    "verification_ran": false,
    "verification_passed": false,
    "changed_files": ["company-service/src/orders/retry.js"],
    "violations": ["verification_missing"]
  },
  "memory": {
    "enabled": true,
    "cards": 0,
    "used_ids": []
  }
}
JSON
cat > target/generated/code-runs/day-failure/output.json <<'JSON'
{
  "edit": {
    "final_success": false,
    "patch_applied": true,
    "changed_files": ["company-service/src/orders/retry.js"],
    "rationale": "failed verification gate"
  }
}
JSON
cat > target/generated/code-runs/day-failure/trace.jsonl <<'JSONL'
{"schema":"air.trace.v1","action":"model_call_start","input":{"task":"Fix the order retry bug and verify it."},"meta":{"model":"code_edit_decider"}}
{"schema":"air.trace.v1","action":"tool_call","input":{"tool":"edit"},"output":{"success":true,"changed_files":["company-service/src/orders/retry.js"]}}
{"schema":"air.trace.v1","action":"model_call","output":{"complete":true,"tool_calls":[]}}
JSONL

"$AIR_BIN" dream run --full --from target/generated --mode evolution --write-regressions \
  --out-dir .air/dream/e2e > target/generated/dream.stdout.json

jq -e '.schema == "air.dream.v1" and .window_inputs >= 2 and .window_artifacts >= 2 and .findings >= 1 and .memory.cards_written >= 1 and .memory.episodes_written >= 2' \
  .air/dream/e2e/dream.json >/dev/null
jq -e '.findings[0].category == "verification_failed"' .air/dream/e2e/improve/findings.json >/dev/null
jq -e '.kind == "code_run_verdict" and .category == "verification_failed"' \
  .air/dream/e2e/improve/suggested-regressions/imp-001.json >/dev/null
jq -e '.candidates | length >= 1' .air/dream/e2e/memory/dream_ir.json >/dev/null
jq -e '[.cards[] | select(.kind == "procedure" and .status == "candidate")] | length >= 1' \
  <("$AIR_BIN" memory list --memory-dir .air/memory) >/dev/null

cat <<EOF
AIR Dream E2E passed.
Provider mode: $(if [[ "$REAL_PROVIDER" == "1" ]]; then echo "real"; else echo "fixture"; fi)
air run exit status: $RUN_STATUS
Scenario: $SCENARIO
Dream report: $SCENARIO/.air/dream/e2e/dream.md
Memory report: $SCENARIO/.air/dream/e2e/memory/report.md
Top finding: $(jq -r '.top_finding.id + " " + .top_finding.category' .air/dream/e2e/dream.json)
Memory cards: $(jq -r '.memory.cards_written' .air/dream/e2e/dream.json)
Run verdict: final_success=$(jq -r '.verdict.final_success // false' target/generated/code-runs/day-success/artifact.json), verification_passed=$(jq -r '.verdict.verification_passed // false' target/generated/code-runs/day-success/artifact.json), patch_applied=$(jq -r '.verdict.patch_applied // false' target/generated/code-runs/day-success/artifact.json)
EOF
