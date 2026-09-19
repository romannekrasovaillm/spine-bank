#!/usr/bin/env bash
# Прогон red-team: копия кейса → git-база (эталон) → overlay (дефект) → команда-проба.
set -u
CASE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-/tmp/red-team-run}"
for v in "$CASE"/red-team/[0-9]*/; do
  name="$(basename "$v")"
  rm -rf "$OUT/$name"; mkdir -p "$OUT/$name"
  cp -a "$CASE/." "$OUT/$name/"
  rm -rf "$OUT/$name/.git"
  ( cd "$OUT/$name" \
    && git init -q . \
    && git -c user.email=rt@local -c user.name=redteam add -A \
    && git -c user.email=rt@local -c user.name=redteam commit -q -m "base: эталон кейса" \
    && cp -a "$v/overlay/." . \
    && cmd="$(cat "$v/probe.txt")" \
    && echo "########## $name" \
    && echo "  проба: $cmd" \
    && eval "$cmd" > /tmp/rt-out.txt 2>&1; echo "  exit=$?"; tail -5 /tmp/rt-out.txt | sed 's/^/  /' )
  echo
done
