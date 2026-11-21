#!/usr/bin/env bash
# Classifies a change by the paths it touches (change `faster-ci`), read one
# per line on standard input, and prints the widest class:
#
#   docs        documentation, specs and images only: nothing is compiled;
#   host-tools  also host-only tools the kernel never links (`cli/`);
#   code        anything else (crates, kernel, xtask, cells, workflows,
#               Cargo files, the toolchain...). No paths at all is `code`.
#
# CI runs the jobs of the class (.github/workflows/ci.yml).
set -euo pipefail

class=0 # 1 docs, 2 host-tools, 3 code
seen=0
while IFS= read -r path; do
  [ -n "$path" ] || continue
  seen=1
  case "$path" in
    docs/* | openspec/* | .agents/* | bench/*.svg | *.md) c=1 ;;
    cli/*) c=2 ;;
    *) c=3 ;;
  esac
  [ "$c" -le "$class" ] || class=$c
done
[ "$seen" = 1 ] || class=3
case $class in
  1) echo docs ;;
  2) echo host-tools ;;
  *) echo code ;;
esac
