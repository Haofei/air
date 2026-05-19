#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCENARIO="${AIR_E2E_DREAM_MULTI_DIR:-/tmp/air-dream-e2e-multi-task}"
AIR_BIN="${AIR_BIN:-$ROOT/target/debug/air}"
MODEL_CONFIG="$SCENARIO/.air/e2e-model-fixtures.json"

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required for this E2E test" >&2
  exit 2
fi

cargo build -q -p air-cli

rm -rf "$SCENARIO"
mkdir -p "$SCENARIO/skills" "$SCENARIO/company-service/src" \
  "$SCENARIO/company-service/test" "$SCENARIO/target/generated/code-runs" \
  "$SCENARIO/.air/memory/cards/procedure"
cp -R "$ROOT/skills/code-agent" "$SCENARIO/skills/code-agent"

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

cat > "$MODEL_CONFIG" <<'JSON'
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

cd "$SCENARIO"
git init -q
printf '.air/\ntarget/\n' > .gitignore
git add .gitignore company-service skills modules
GIT_AUTHOR_NAME='AIR E2E' GIT_AUTHOR_EMAIL='air-e2e@example.com' \
  GIT_COMMITTER_NAME='AIR E2E' GIT_COMMITTER_EMAIL='air-e2e@example.com' \
  git commit -qm 'seed multi-task company service fixture'

"$AIR_BIN" run --mode code --no-memory \
  --model-config "$MODEL_CONFIG" \
  --tool-config skills/code-agent/tools.json \
  --artifact-out target/generated/code-runs/day-01-billing-success \
  "Fix the billing cents addition bug and run the appropriate test." \
  > target/generated/day-01-billing-success.run.json

if [[ ! -f target/generated/code-runs/day-01-billing-success/artifact.json ]]; then
  echo "billing run did not produce an artifact" >&2
  exit 1
fi

cat > .air/memory/cards/procedure/mem_seed_verification.json <<'JSON'
{
  "schema": "air.memory_card.v1",
  "id": "mem_seed_verification",
  "kind": "procedure",
  "scope": {
    "level": "repo",
    "repo": "air-dream-e2e-multi-task",
    "executor": "code-agent"
  },
  "title": "Verification-first JavaScript bugfix",
  "content": "For JavaScript service bugfixes, run the focused node test before editing, patch the smallest source file, then rerun the same focused test before reporting success.",
  "triggers": ["javascript", "verification", "node test", "bugfix"],
  "evidence": [
    {
      "kind": "seeded_e2e_memory",
      "path": "target/generated/code-runs/day-01-billing-success/artifact.json",
      "verdict": "success"
    }
  ],
  "confidence": 0.9,
  "impact": 0.8,
  "stability": "medium",
  "status": "promoted",
  "created_at": 1770001000,
  "updated_at": 1770001000,
  "conflicts": [],
  "promotion": {
    "can_prompt_inject": true,
    "can_route": true,
    "can_compile_skill": true,
    "can_regression": false,
    "can_policy": false,
    "requires_human_review": false
  }
}
JSON

write_artifact() {
  local name="$1"
  local task="$2"
  local final_success="$3"
  local category="$4"
  local reason="$5"
  local patch_applied="$6"
  local verification_ran="$7"
  local verification_passed="$8"
  local changed_file="$9"
  local memory_id="${10:-}"
  local extra="{}"
  if [[ -n "$memory_id" ]]; then
    extra="$(jq -nc --arg id "$memory_id" '{memory_pack:{cards:[{id:$id,kind:"procedure",title:"Verification-first JavaScript bugfix"}]}}')"
  fi
  local dir="target/generated/code-runs/$name"
  mkdir -p "$dir"
  cat > "$dir/artifact.json" <<JSON
{
  "schema": "air.code_run_artifact.v1",
  "task": "$task",
  "descriptor": {
    "task": "$task",
    "cwd": "$SCENARIO",
    "trace": "trace.jsonl",
    "outputs": "output.json",
    "created_at_unix": 1770001000,
    "constraints": {
      "require_patch": true,
      "require_verification": true
    }
  },
  "skill": { "id": "code-agent" },
  "delta": {
    "changed_files": $(if [[ -n "$changed_file" ]]; then printf '["%s"]' "$changed_file"; else printf '[]'; fi)
  },
  "failure_reason": $(if [[ "$final_success" == "true" ]]; then printf 'null'; else printf '{"category":"%s","message":"%s"}' "$category" "$reason"; fi),
  "verdict": {
    "schema": "air.code_run_verdict.v1",
    "final_success": $final_success,
    "category": "$category",
    "reason": "$reason",
    "patch_applied": $patch_applied,
    "verification_ran": $verification_ran,
    "verification_passed": $verification_passed,
    "allowed_files_ok": true,
    "required_files_ok": $(if [[ "$category" == "diff_constraint_failed" ]]; then printf 'false'; else printf 'true'; fi),
    "forbidden_files_ok": true,
    "required_diff_ok": $(if [[ "$category" == "diff_constraint_failed" ]]; then printf 'false'; else printf 'true'; fi),
    "max_diff_lines_ok": true,
    "changed_files": $(if [[ -n "$changed_file" ]]; then printf '["%s"]' "$changed_file"; else printf '[]'; fi),
    "violations": $(if [[ "$final_success" == "true" ]]; then printf '[]'; else printf '["%s"]' "$category"; fi)
  },
  "extra": $extra
}
JSON
  cat > "$dir/output.json" <<JSON
{
  "edit": {
    "final_success": $final_success,
    "patch_applied": $patch_applied,
    "changed_files": $(if [[ -n "$changed_file" ]]; then printf '["%s"]' "$changed_file"; else printf '[]'; fi),
    "rationale": "$reason"
  }
}
JSON
  cat > "$dir/trace.jsonl" <<JSONL
{"schema":"air.trace.v1","action":"model_call_start","input":{"task":"$task"},"meta":{"model":"code_edit_decider"}}
{"schema":"air.trace.v1","action":"tool_call","input":{"tool":"bash"},"output":{"success":$verification_passed}}
{"schema":"air.trace.v1","action":"model_call","output":{"complete":true,"tool_calls":[]}}
JSONL
}

write_artifact \
  "day-02-tax-success" \
  "Fix sales tax rounding and verify the tax test." \
  "true" "success" "focused verification passed" \
  "true" "true" "true" \
  "company-service/src/tax.js"

write_artifact \
  "day-03-memory-guided-success" \
  "Fix invoice total formatting using the verification-first memory." \
  "true" "success" "memory-guided focused verification passed" \
  "true" "true" "true" \
  "company-service/src/invoice.js" \
  "mem_seed_verification"

write_artifact \
  "day-04-no-patch-failure" \
  "Fix the customer display name bug." \
  "false" "no_patch_applied" "agent searched and verified but never edited a file" \
  "false" "true" "true" \
  ""

write_artifact \
  "day-05-budget-failure" \
  "Refactor the catalog discount rules without exceeding the tool budget." \
  "false" "budget_exceeded" "agent exhausted model/tool budget during broad exploration" \
  "false" "false" "false" \
  ""

write_artifact \
  "day-06-diff-constraint-failure" \
  "Fix the fraud threshold but only modify company-service/src/fraud.js." \
  "false" "diff_constraint_failed" "candidate changed an unexpected config file and missed required diff constraints" \
  "true" "true" "true" \
  "company-service/config/fraud.json"

write_artifact \
  "day-07-verification-failure" \
  "Fix the order retry bug and verify it." \
  "false" "verification_failed" "agent patched retry logic but did not produce passing verification" \
  "true" "false" "false" \
  "company-service/src/orders/retry.js"

"$AIR_BIN" dream run --full --from target/generated --mode evolution --write-regressions \
  --out-dir .air/dream/multi > target/generated/dream-multi.stdout.json

jq -e '.schema == "air.dream.v1" and .window_artifacts >= 7 and .findings >= 4 and .memory.cards_written >= 12 and .memory_advance.routing_measurements >= 1 and .memory_advance.reviewed_guards >= 1' \
  .air/dream/multi/dream.json >/dev/null
jq -e '[.findings[].category] | index("budget_exceeded") and index("diff_constraint_failed") and index("no_patch_applied") and index("verification_failed")' \
  .air/dream/multi/improve/findings.json >/dev/null
jq -e '.candidates | length >= 3' .air/dream/multi/memory/dream_ir.json >/dev/null
jq -e '[.cards[] | select(.id == "mem_seed_verification" and .helped_candidate >= 1)] | length == 1' \
  <("$AIR_BIN" memory scorecard --memory-dir .air/memory) >/dev/null
jq -e '.summary.memory.promoted == 1 and .summary.skills.validated_skill == 1 and .summary.policies.guard_proposals == 4 and .summary.findings.open == 4' \
  <("$AIR_BIN" brain --json --memory-dir .air/memory --dream-dir .air/dream/multi) >/dev/null
jq -e '.skills.drafts[] | select(.id == "verification-first-javascript-bugfix" and .status == "validated_skill")' \
  <("$AIR_BIN" brain skills --json --memory-dir .air/memory --dream-dir .air/dream/multi) >/dev/null
jq -e '.policies.guard_proposals | length == 4' \
  <("$AIR_BIN" brain policies --json --memory-dir .air/memory --dream-dir .air/dream/multi) >/dev/null
"$AIR_BIN" brain report --json --memory-dir .air/memory --dream-dir .air/dream/multi --out .air/brain/multi-report.md \
  > target/generated/brain-report.stdout.json
test -f .air/brain/multi-report.md

cat <<EOF
AIR Dream multi-task E2E passed.
Scenario: $SCENARIO
Dream report: $SCENARIO/.air/dream/multi/dream.md
Memory report: $SCENARIO/.air/dream/multi/memory/report.md
Advance report: $SCENARIO/.air/dream/multi/memory-advance/advance.md
Window artifacts: $(jq -r '.window_artifacts' .air/dream/multi/dream.json)
Findings: $(jq -r '.findings' .air/dream/multi/dream.json)
Top finding: $(jq -r '.top_finding.id + " " + .top_finding.category + " score=" + (.top_finding.impact_score|tostring)' .air/dream/multi/dream.json)
Finding categories: $(jq -r '[.findings[].category] | join(", ")' .air/dream/multi/improve/findings.json)
Memory cards: $(jq -r '.memory.cards_written' .air/dream/multi/dream.json)
Dream IR candidates: $(jq -r '.candidates | length' .air/dream/multi/memory/dream_ir.json)
Routing measurements: $(jq -r '.memory_advance.routing_measurements' .air/dream/multi/dream.json)
Guard proposals: $(jq -r '.memory_advance.reviewed_guards' .air/dream/multi/dream.json)
Seed memory helped_candidate: $(jq -r '.cards[] | select(.id == "mem_seed_verification") | .helped_candidate' <("$AIR_BIN" memory scorecard --memory-dir .air/memory))
Brain report: $SCENARIO/.air/brain/multi-report.md
EOF
