#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${NEEDS_JSON:-}" ]]; then
  echo "::error::NEEDS_JSON is required"
  exit 1
fi

if jq -e '
  [to_entries[] | .value.result | select(. == "failure" or . == "cancelled")]
  | length > 0
' <<<"$NEEDS_JSON" >/dev/null; then
  echo "::error::One or more CI jobs failed or were cancelled"
  exit 1
fi
