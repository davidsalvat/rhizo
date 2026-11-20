#!/usr/bin/env bash
# Publishes pieces FIRST to FIRST+8 of bench-results.json (compact JSON in
# 3000-character pieces) as annotations bench-results-N: a step shows at
# most 10 notices, so the CI job calls this once per nine pieces.
set -euo pipefail
first=$1
[ -f bench-results.json ] || exit 0
jq -c . bench-results.json | tr -d '\n' | fold -w 3000 | nl -nln -w1 -s' ' |
  while IFS= read -r line; do
    n=${line%% *}
    if [ "$n" -ge "$first" ] && [ "$n" -lt $((first + 9)) ]; then
      echo "::notice title=bench-results-$n::${line#* }"
    fi
  done
