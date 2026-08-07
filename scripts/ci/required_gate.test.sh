#!/usr/bin/env bash
set -euo pipefail

run_gate() {
  NEEDS_JSON="$1" bash scripts/ci/required_gate.sh >/dev/null 2>&1
}

for result in failure cancelled; do
  needs_json="$(jq -cn --arg result "$result" '{job: {result: $result}}')"
  if run_gate "$needs_json"; then
    echo "required gate must fail for $result result"
    exit 1
  fi
done

run_gate '{"job":{"result":"success"}}'
run_gate '{"job":{"result":"skipped"}}'

if NEEDS_JSON='' bash scripts/ci/required_gate.sh >/dev/null 2>&1; then
  echo "required gate must fail without needs input"
  exit 1
fi

echo "required gate aggregation tests passed"
