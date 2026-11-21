#!/usr/bin/env bash
# Tests for classify.sh: each case is the expected class and the paths.
set -euo pipefail
here=$(dirname "$0")
fail=0
check() {
  local want=$1
  shift
  local got
  got=$(printf '%s\n' "$@" | "$here/classify.sh")
  if [ "$got" != "$want" ]; then
    echo "FAILED classify: expected $want, got $got for: $*"
    fail=1
  fi
}
check docs README.md
check docs docs/security.md docs/img/gen.py docs/img/check.js docs/img/hero.svg
check docs openspec/changes/faster-ci/proposal.md openspec/specs/benchmarks/spec.md
check docs .agents/skills/benchmarks/SKILL.md AGENTS.md bench/README.md
check docs bench/compare.svg
check host-tools cli/src/main.rs
check host-tools cli/Cargo.toml README.md
check code crates/rhizo-mm/src/heap.rs
check code kernel/src/cell.rs
check code xtask/src/bench.rs
check code cells/c/hello.c
check code .github/workflows/ci.yml
check code .github/scripts/classify.sh
check code Cargo.lock
check code Cargo.toml
check code rust-toolchain.toml
check code bench/baseline.json
check code cli/src/main.rs crates/rhizo-control/src/lib.rs
check code docs/security.md kernel/src/tests.rs
check code ""
check code
if [ "$fail" = 0 ]; then echo "classify: every case passes"; fi
exit "$fail"
