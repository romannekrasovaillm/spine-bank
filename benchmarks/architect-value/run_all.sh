#!/usr/bin/env bash
# run_all.sh — прогон бенчмарка architect-value (P1) и публикация результатов.
#
# Сьюты (метрики §9 задачи):
#   StdComply        — 7, 9, 10 (control check стандарта/манифесты) + 8 (семена C.2/C.3)
#   arch-diff-replay — 13 (replay.py)
#   MCP              — 15, 16 (mcp_schema_probe.py)
#   CONTRACT         — семантические зонды контрактов (AB-12, ADR-079, semantic_diff.py)
#   ArchRecover      — 1 (survey_compare.py), 2 (components_recover.py),
#                      3 (edges_recover.py), 18/19 (recover_determinism.py, волна R/R5),
#                      4 (stores_recover.py), 5 (externals_recover.py), 6 (survey_noise.py)
#   Kontur           — 11, 12 (kontur_bench.py); 20 — SKIP (судья)
#   selftest/judge   — 14, 17 — SKIP (вне Python-прогонщиков)
#
# Вход: --repos-dir <путь> (иначе $ARCH_VALUE_REPOS, иначе ~/bench-repos);
#       $ARCH_BE — бинарь arch-be; $ARCH_VALUE_OUT — корень вывода (иначе results/).
# Выход: results/<версия>-<дата>/{run_all.log,summary.md}; exit 0 (измерение).
# SKIP — не «зелень»: честно печатается причина (волна не реализована / нет клона).
set -u

BASE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNNERS="$BASE/runners"
REPOS="${ARCH_VALUE_REPOS:-$HOME/bench-repos}"
ARCH_BE="${ARCH_BE:-arch-be}"
OUT_ROOT="${ARCH_VALUE_OUT:-$BASE/results}"

while [ $# -gt 0 ]; do
  case "$1" in
    --repos-dir) REPOS="$2"; shift 2 ;;
    --out)       OUT_ROOT="$2"; shift 2 ;;
    *) echo "неизвестный аргумент: $1" >&2; exit 2 ;;
  esac
done

VER="$("$ARCH_BE" --version 2>/dev/null | tr -d '\n' | sed 's/^[^0-9]*//' | cut -d' ' -f1)"
[ -n "$VER" ] || VER="unknown"
DATE="$(date -u +%Y-%m-%d)"
DEST="$OUT_ROOT/${VER}-${DATE}"
mkdir -p "$DEST"
LOG="$DEST/run_all.log"
PY="${PYTHON:-python3}"

# AD-7 (личные пути не живут в репозитории): лог санитизируется —
# домашний путь исполнителя заменяется плейсхолдером /home/user
sanitize() { sed -e "s|$HOME|/home/user|g" -e 's|/home/[a-z][a-z0-9_-]*|/home/user|g'; }

{
  echo "# architect-value — run_all"
  echo "version: $VER"
  echo "date: $DATE"
  echo "arch_be: $ARCH_BE"
  echo "repos_dir: $REPOS"
  echo
} | sanitize | tee "$LOG"

run() {  # run <заголовок> <runner.py> [аргументы...]
  local title="$1"; shift
  echo "## $title" | sanitize | tee -a "$LOG"
  "$PY" "$RUNNERS/$1" --repos-dir "$REPOS" 2>&1 | sanitize | tee -a "$LOG"
  echo | sanitize | tee -a "$LOG"
}

skip() {  # skip <метрики> <причина>
  echo "[SKIP] $1: $2" | sanitize | tee -a "$LOG"
}

echo "## Предполёт: разбор входов (test_parsers)" | sanitize | tee -a "$LOG"
"$PY" "$RUNNERS/test_parsers.py" 2>&1 | sanitize | tee -a "$LOG"
echo | sanitize | tee -a "$LOG"

# ── StdComply: стандарт + семена ────────────────────────────────────────────
run "StdComply — стандарт Java-сервисы (метрики 7, 9, 10)" control_standard.py
run "StdComply — семена buckpal (метрика 8, C.3)" seed_buckpal.py

# ── arch-diff-replay ────────────────────────────────────────────────────────
run "arch-diff-replay — значимые диффы и окна-шума (метрика 13)" replay.py

# ── MCP ─────────────────────────────────────────────────────────────────────
run "MCP — схема ↔ реализация (метрики 15, 16)" mcp_schema_probe.py

# ── CONTRACT: семантические зонды контрактов (AB-12, ADR-079) ───────────────
run "CONTRACT — семантические зонды контрактов (AB-12, ADR-079)" semantic_diff.py

# ── ArchRecover: survey-детерминизм + рёбра + компоненты + recover ──────────
run "ArchRecover — survey . ≡ survey <abs> (метрика 1)" survey_compare.py
run "ArchRecover — компоненты F1 на пиннутом SHA (метрика 2)" components_recover.py
run "ArchRecover — рёбра as-built recall/precision (метрика 3)" edges_recover.py
run "ArchRecover — детерминизм и время recover (метрики 18, 19)" recover_determinism.py
run "ArchRecover — хранилища as-built (метрика 4: recall)" stores_recover.py
run "ArchRecover — внешние системы as-built (метрика 5: precision)" externals_recover.py
run "ArchRecover — шум карты обследования (метрика 6)" survey_noise.py

# ── Kontur ──────────────────────────────────────────────────────────────────
echo "## Kontur — измеримые кейсы и FP на реальном коде (метрики 11, 12, 20)" | sanitize | tee -a "$LOG"
run "Kontur — измеримость кейсов и FP на реальном коде (метрики 11, 12)" kontur_bench.py

skip "metric 20 (квалификация судьи: различительная сила)" "рубрики/судья — вне Python-прогонщиков"
echo | sanitize | tee -a "$LOG"

# ── selftest / golden judge ─────────────────────────────────────────────────
echo "## Прочее (вне Python-прогонщиков)" | sanitize | tee -a "$LOG"
skip "metric 14 (selftest из коробки)" "измеряется в CI (cargo test), не прогонщиком бенчмарка"
skip "metric 17 (golden judge MAE / доля в диапазоне)" "требует API-ключ судьи — вне офлайн-прогона"
echo | sanitize | tee -a "$LOG"

# ── Сводка ──────────────────────────────────────────────────────────────────
{
  echo "# Сводка прогона ${VER}-${DATE}"
  echo
  echo "Каталог: \`results/${VER}-${DATE}/\` (полный вывод — \`run_all.log\`)."
  echo "Прогон: \`run_all.sh\`; SKIP ≠ PASS (см. \`PREREGISTRATION.md\` §5)."
  echo
  echo "## Итоги по метрикам"
  echo
  echo '```'
  grep -E '^\[(RUN|SKIP|FIND)\]' "$LOG" || true
  echo '```'
  echo
  echo "## Счётчики"
  echo
  echo "| маркер | число |"
  echo "|--------|-------|"
  echo "| RUN    | $(grep -cE '^\[RUN\]' "$LOG") |"
  echo "| SKIP   | $(grep -cE '^\[SKIP\]' "$LOG") |"
  echo "| FIND   | $(grep -cE '^\[FIND\]' "$LOG") |"
} > "$DEST/summary.md"

echo "готово: $DEST (run_all.log, summary.md)" | sanitize | tee -a "$LOG"
exit 0
