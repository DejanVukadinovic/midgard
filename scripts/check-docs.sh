#!/usr/bin/env bash
# Documentation checks (OSS-07): relative links resolve, every `make <target>` mentioned in docs
# exists, and referenced scripts exist. The quickstart itself is executed end-to-end in CI.
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0
mapfile -t docs < <(git ls-files -co --exclude-standard '*.md' | grep -v '^requirements/')
for f in "${docs[@]}"; do
  [[ -f "$f" ]] || continue
  dir="$(dirname "$f")"
  # relative links: [text](path) excluding URLs and anchors
  while IFS= read -r link; do
    path="${link%%#*}"
    [[ -z "$path" ]] && continue
    [[ -e "$dir/$path" ]] || { echo "BROKEN LINK in $f: $link"; fail=1; }
  done < <(grep -oE '\]\([^)]+\)' "$f" | sed -E 's/^\]\((.*)\)$/\1/' | grep -vE '^(https?:|mailto:|#)' || true)
  # make targets
  while IFS= read -r target; do
    grep -qE "^${target}:" Makefile || { echo "UNKNOWN make target in $f: make $target"; fail=1; }
  done < <(grep -oE '(^|[`$]|&& |; |\| )make [a-z][a-z-]*' "$f" | awk '{print $NF}' | sort -u || true)
  # scripts
  while IFS= read -r s; do
    [[ -e "$s" ]] || { echo "MISSING script referenced in $f: $s"; fail=1; }
  done < <(grep -oE '(scripts|tests/acceptance|deployments/compose)/[A-Za-z0-9_./-]+\.sh' "$f" | sort -u || true)
done
[[ $fail == 0 ]] && echo "docs OK"
exit $fail
