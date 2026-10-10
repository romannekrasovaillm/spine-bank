#!/usr/bin/env bash
# run_all.sh — прогон бенчмарка architect-value (P1) и публикация результатов.
#
# Сьюты (метрики §9 задачи):
#   StdComply        — 7, 9, 10 (control check стандарта/манифесты) + 8 (семена C.2/C.3)
#   arch-diff-replay — 13 (replay.py)
#   MCP              — 15, 16 (mcp_schema_probe.py)
#   ArchRecover      — 1 (survey_compare.py); 2–6, 18, 19 — SKIP (волна R не в 0.3.17)
#   Kontur           — 11, 12, 20 — SKIP (волна K не в 0.3.17)
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

{
  echo "# architect-value — run_all"
  echo "version: $VER"
  echo "date: $DATE"
  echo "arch_be: $ARCH_BE"
  echo "repos_dir: $REPOS"
  echo
} | tee "$LOG"

run() {  # run <заголовок> <runner.py> [аргументы...]
  local title="$1"; shift
  echo "## $title" | tee -a "$LOG"
  "$PY" "$RUNNERS/$1" --repos-dir "$REPOS" 2>&1 | tee -a "$LOG"
  echo | tee -a "$LOG"
}

skip() {  # skip <метрики> <причина>
  echo "[SKIP] $1: $2" | tee -a "$LOG"
}

echo "## Предполёт: разбор входов (test_parsers)" | tee -a "$LOG"
"$PY" "$RUNNERS/test_parsers.py" 2>&1 | tee -a "$LOG"
echo | tee -a "$LOG"

# ── StdComply: стандарт + семена ────────────────────────────────────────────
run "StdComply — стандарт Java-сервисы (метрики 7, 9, 10)" control_standard.py
run "StdComply — семена buckpal (метрика 8, C.3)" seed_buckpal.py

# ── arch-diff-replay ────────────────────────────────────────────────────────
run "arch-diff-replay — значимые диффы и окна-шума (метрика 13)" replay.py

# ── MCP ─────────────────────────────────────────────────────────────────────
run "MCP — схема ↔ реализация (метрики 15, 16)" mcp_schema_probe.py

# ── ArchRecover: survey-детерминизм + SKIP волны R ──────────────────────────
run "ArchRecover — survey . ≡ survey <abs> (метрика 1)" survey_compare.py
{
  echo "## ArchRecover — восстановление архитектуры (метрики 2–6, 18, 19)"
} | tee -a "$LOG"
skip "metric 2 (компоненты F1)"   "волна R (recover) не реализована в 0.3.17"
skip "metric 3 (рёбра recall/precision)" "волна R (recover) не реализована в 0.3.17"
skip "metric 4 (хранилища recall)" "волна R (recover) не реализована в 0.3.17"
skip "metric 5 (внешние precision)" "волна R (recover) не реализована в 0.3.17"
skip "metric 6 (шум карты survey)" "механизм грунт-труса recover отсутствует в 0.3.17"
skip "metric 18 (детерминизм recover/arch-diff JSON)" "recover отсутствует; arch-diff даёт JSON (см. metric 13)"
skip "metric 19 (время recover ≤5 тыс. файлов)" "recover отсутствует в 0.3.17"
echo | tee -a "$LOG"

# ── Kontur ──────────────────────────────────────────────────────────────────
echo "## Kontur — измеримые кейсы и FP на реальном коде (метрики 11, 12, 20)" | tee -a "$LOG"
skip "metric 11 (Kontur-bench измеримых кейсов)" "волна K (redteam) не реализована в 0.3.17"
skip "metric 12 (Kontur на реальном коде: detection/FP)" "волна K (redteam) не реализована в 0.3.17"
skip "metric 20 (квалификация судьи: различительная сила)" "рубрики/судья — вне Python-прогонщиков"
echo | tee -a "$LOG"

# ── selftest / golden judge ─────────────────────────────────────────────────
echo "## Прочее (вне Python-прогонщиков)" | tee -a "$LOG"
skip "metric 14 (selftest из коробки)" "измеряется в CI (cargo test), не прогонщиком бенчмарка"
skip "metric 17 (golden judge MAE / доля в диапазоне)" "требует API-ключ судьи — вне офлайн-прогона"
echo | tee -a "$LOG"

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

echo "готово: $DEST (run_all.log, summary.md)" | tee -a "$LOG"
exit 0
