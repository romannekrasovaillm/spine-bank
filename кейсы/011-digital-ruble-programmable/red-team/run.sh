#!/usr/bin/env bash
# Прогон red-team вариантов: копия кейса + overlay + команда-проба.
set -u
CASE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-/tmp/red-team-run}"
for v in "$CASE"/red-team/[0-9]*/; do
  name="$(basename "$v")"
  rm -rf "$OUT/$name"; mkdir -p "$OUT/$name"
  cp -a "$CASE/." "$OUT/$name/"
  cp -a "$v/overlay/." "$OUT/$name/"
  cmd="$(cat "$v/probe.txt")"
  echo "########## $name"
  echo "  проба: $cmd"
  ( cd "$OUT/$name" && eval "$cmd" ) 2>&1 | tail -6 | sed 's/^/  /'
  code=${PIPESTATUS[0]}
  echo "  exit=$code"
  echo
done
