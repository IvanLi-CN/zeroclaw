#!/usr/bin/env bash
set -euo pipefail

gate_fails_for() {
  local result="$1"
  local needs_json
  needs_json="$(jq -cn --arg result "$result" '{job: {result: $result}}')"
  ! jq -e '
    [to_entries[] | .value.result | select(. == "failure" or . == "cancelled")]
    | length > 0
  ' <<<"$needs_json" >/dev/null
}

gate_fails_for failure && {
  echo "required gate must fail for failure result"
  exit 1
}
gate_fails_for cancelled && {
  echo "required gate must fail for cancelled result"
  exit 1
}

skipped_json='{"job":{"result":"skipped"}}'
if jq -e '
  [to_entries[] | .value.result | select(. == "failure" or . == "cancelled")]
  | length > 0
' <<<"$skipped_json" >/dev/null; then
  echo "required gate must tolerate intentionally skipped jobs"
  exit 1
fi

echo "required gate aggregation tests passed"
