#!/usr/bin/env bash
# run_all.sh — прогон бенчмарка benchmarks/icl-transfer (пререгистрация 0.5.0).
#
# Режимы:
#   --ci   (по умолчанию) детерминированный прогон механики: FakeLlm +
#          fake-harness, без сети, в cargo test. Руки FI+L, FI, X, GEN, PAIR,
#          PROMO, FLEET. Выход: results/<run>/ci-summary.json + ci-report.md.
#   --live нативный прогон (ключи [models] или шим) — в BENCH-1 НЕ реализован,
#          печатается честный SKIP с причиной.
#
# Пороги заморожены в PREREGISTRATION.md §4 и здесь НЕ подкручиваются.
set -u

BASE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNNERS="$BASE/runners"
PY="${PYTHON:-python3}"
OUT_ROOT="${ICL_OUT:-$BASE/results}"
MODE="--ci"

while [ $# -gt 0 ]; do
  case "$1" in
    --ci)   MODE="--ci"; shift ;;
    --live) MODE="--live"; shift ;;
    --out)  OUT_ROOT="$2"; shift 2 ;;
    *) echo "неизвестный аргумент: $1" >&2; exit 2 ;;
  esac
done

# AD-7: личные пути не живут в отчётах — домашний путь заменяется плейсхолдером.
sanitize() { sed -e "s|$HOME|/home/user|g" -e 's|/home/[a-z][a-z0-9_-]*|/home/user|g'; }

DATE="$(date -u +%Y-%m-%d)"
DEST="$OUT_ROOT/ci-$DATE"
mkdir -p "$DEST"

if [ "$MODE" = "--live" ]; then
  {
    echo "# icl-transfer — run_all (live)"
    echo "[SKIP] --live: нативный прогон вне объёма BENCH-1 (нужны ключи [models] или шим);"
    echo "       реализуется в волне 3 после L8–L10. См. PREREGISTRATION.md §5."
  } | sanitize | tee "$DEST/run.log"
  exit 0
fi

{
  echo "# icl-transfer — run_all (--ci)"
  echo "date: $DATE"
  echo "mode: ci (FakeLlm + fake-harness, детерминированно, без сети)"
  echo
} | sanitize | tee "$DEST/run.log"

"$PY" "$RUNNERS/iclbench.py" --ci --out "$DEST" 2>&1 | sanitize | tee -a "$DEST/run.log"
RC=$?

# Отчёт с метриками и порогами (пороги — из PREREGISTRATION.md §4).
"$PY" - "$DEST/ci-summary.json" "$DEST/ci-report.md" <<'PYEOF' 2>&1 | sanitize | tee -a "$DEST/run.log"
import json, sys
summary = json.load(open(sys.argv[1], encoding="utf-8"))
arms = summary["arms"]
ag = summary["antigoodhart"]

def pct(x):
    return f"{x*100:.0f}%"

rows = []
def row(label, value, threshold, unit="", ok=True):
    rows.append((label, value, threshold, unit, ok))

fil = arms.get("FI+L", {})
fi = arms.get("FI", {})
x = arms.get("X", {})
gen = arms.get("GEN", {})
pair = arms.get("PAIR", {})
promo = arms.get("PROMO", {})
fleet = arms.get("FLEET", {})

row("E1: повтор скрытой конвенции (FI+L, T2–T5)", pct(fil.get("repeat_failure_rate", 1)), "≤ 5%")
row("E1: контроль без памяти (FI, T2–T5)", pct(fi.get("repeat_failure_rate", 0)), "100% (контроль)")
row("E1: первый PASS на T1 второй серии (FI+L)", pct(fil.get("first_pass_t1_series2", 0)), "≥ 80%")
row("E2: перенос на второй исполнитель (X)", pct(x.get("transfer_rate", 0)), "≥ 90%")
row("E3: отложенное семейство (GEN, миграция)", pct(gen.get("migration_compliance", 0)), "≥ 70%")
row("E4: две конвенции одного kind (PAIR)", pct(pair.get("first_pass_after_pair", 0)), "≥ 60%")
row("E5: поймано pre-commit до приёмки (PROMO, 6–10)", pct(promo.get("precommit_caught_rate", 0)), "≥ 50%")
row("E6: повтор у стартовавших после FAIL (FLEET)", pct(fleet.get("late_repeat_rate", 1)), "≤ 20%")
row("Прогонов исполнителя на задачу (FI+L)", f"{fil.get('dispatches_per_task', 0):.2f}", "≤ 1.10")

lines = ["# icl-transfer — сводка --ci", ""]
lines.append("Режим `--ci`: механика обучающего контура проверяется детерминированно")
lines.append("(FakeLlm + fake-harness, без сети). Пороги — `PREREGISTRATION.md` §4.")
lines.append("")
lines.append("| Метрика | Значение | Порог |")
lines.append("|---------|----------|-------|")
for label, value, threshold, unit, ok in rows:
    lines.append(f"| {label} | {value} | {threshold} |")
lines.append("")
lines.append("## Анти-Goodhart")
lines.append(f"Маркеры пула в src/ assets/: **{'PASS' if ag['passed'] else 'FAIL'}**")
if ag.get("violations"):
    for marker, path in ag["violations"]:
        lines.append(f"- `{marker}` в `{path}`")
lines.append("")
lines.append("## Выбранные конвенции (по первому семени руки)")
for arm, d in arms.items():
    lines.append(f"- {arm}: {', '.join(d.get('selected', []))}")
lines.append("")
open(sys.argv[2], "w", encoding="utf-8").write("\n".join(lines) + "\n")
print("отчёт:", sys.argv[2])
PYEOF

echo "готово: $DEST (ci-summary.json, ci-report.md)" | sanitize | tee -a "$DEST/run.log"
exit "$RC"
