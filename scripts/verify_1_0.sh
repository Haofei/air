#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

HELP_OUT="target/generated/air_help.txt"
PYTHON="${AIR_PYTHON:-.venv/bin/python}"
mkdir -p target/generated

echo "[air-1.0] public CLI surface"
cargo run -q -p air-cli -- --help > "$HELP_OUT"
grep -q "validate-plan" "$HELP_OUT"
grep -q "run-plan" "$HELP_OUT"
grep -q "resume-plan" "$HELP_OUT"
grep -q "lower-plan" "$HELP_OUT"
grep -q "plan" "$HELP_OUT"
if grep -Eq "validate-system|run-system| replay| lower " "$HELP_OUT"; then
  echo "hidden developer commands leaked into public help" >&2
  cat "$HELP_OUT" >&2
  exit 1
fi

echo "[air-1.0] docs"
test -f docs/user_guide.md
test -f docs/air_1_0_audit.md
test -f docs/condition_dsl.md
grep -q "scripts/verify_1_0.sh" docs/user_guide.md
grep -q "Current completion" docs/air_1_0_audit.md
grep -q "condition  :=" docs/condition_dsl.md

echo "[air-1.0] packaged examples"
cargo run -q -p air-cli -- validate-plan --profile examples/simple-helpdesk/profile.air-profile.yaml
cargo run -q -p air-cli -- validate-plan --profile examples/deep-research/profile.air-profile.yaml

echo "[air-1.0] HTTP JSON tool provider smoke"
cargo run -q -p air-cli -- validate tests/agents/http-json-tool-smoke.air.yaml
cargo run -q -p air-cli -- validate-plan tests/plans/http-json-tool-smoke.air-plan.yaml \
  --store tests/plans/http-json-tool-smoke.air-store.yaml
HTTP_TOOL_PORT="$("$PYTHON" - <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"
AIR_HTTP_TOOL_PORT="$HTTP_TOOL_PORT" "$PYTHON" scripts/http_json_tool_smoke_server.py \
  > target/generated/http_json_tool_smoke.server.log &
HTTP_TOOL_PID="$!"
trap 'kill "$HTTP_TOOL_PID" 2>/dev/null || true' EXIT
sleep 0.2
sed "s/{{PORT}}/${HTTP_TOOL_PORT}/g" tests/plans/http-json-tool-smoke.tools.template.json \
  > target/generated/http_json_tool_smoke.tools.json
cargo run -q -p air-cli -- run-plan tests/plans/http-json-tool-smoke.air-plan.yaml \
  --store tests/plans/http-json-tool-smoke.air-store.yaml \
  --input tests/plans/http-json-tool-smoke.input.json \
  --tool-config target/generated/http_json_tool_smoke.tools.json \
  --trace-out target/generated/http_json_tool_smoke.native.trace.jsonl \
  > target/generated/http_json_tool_smoke.native.output.json
"$PYTHON" - <<'PY'
import json

with open("target/generated/http_json_tool_smoke.native.output.json", encoding="utf-8") as handle:
    output = json.load(handle)
assert output["result"]["query"] == "air runtime"
assert output["result"]["documents"][0]["id"] == "http-json"

with open("target/generated/http_json_tool_smoke.native.trace.jsonl", encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.lstrip().startswith("{")]
assert any(event["action"] == "tool_call" and event["status"] == "ok" for event in events)
PY
kill "$HTTP_TOOL_PID" 2>/dev/null || true
trap - EXIT

echo "[air-1.0] backend conformance"
scripts/verify_backend_conformance.sh

echo "[air-1.0] scoped verifier"
if [[ "${AIR_1_0_REAL:-0}" == "1" ]]; then
  AIR_DEEP_RESEARCH_REAL=1 scripts/verify_deep_research.sh
else
  scripts/verify_deep_research.sh
fi

echo "[air-1.0] verification complete"
