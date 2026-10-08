#!/usr/bin/env bash
# e2e-прогон кейса openspec-bridge (F7): жизненный цикл change OpenSpec под
# гейтом Spine, на временном git-репозитории (кейс копируется, сеть не нужна).
#
#   bash e2e.sh [путь к arch-be]
#
# Шаги:
#   0. чистое дерево: gate PASS;
#   1. правка model/ в рамках активного change → delta_guard PASS,
#      источник покрытия openspec:add-limits;
#   2. контроль: правка защищённого файла, НЕ упомянутого в change, → FAIL;
#   3. openspec gate --archive: требования дельты покрыты правилами covers:;
#   4. архивирование change (семантика `openspec archive`): каталог уезжает в
#      archive/<дата>-<id>, требование сливается в живую спеку;
#   5. gate на диапазоне с архивацией → PASS: покрытие — архивный change
#      внутри диапазона (openspec:2026-10-08-add-limits);
#   6. openspec coverage --strict: 3/3 требований покрыто и после слияния.

set -euo pipefail

ARCH_BE="${1:-arch-be}"
# Бинарь понадобится после cd во временный репозиторий — абсолютный путь.
if [[ "$ARCH_BE" == */* ]]; then
  ARCH_BE="$(cd "$(dirname "$ARCH_BE")" && pwd)/$(basename "$ARCH_BE")"
else
  ARCH_BE="$(command -v "$ARCH_BE")"
fi
CASE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/repo"
cp -r "$CASE_DIR/openspec" "$CASE_DIR/model" "$CASE_DIR/src" "$WORK/repo/"
cp "$CASE_DIR/CONSTRAINTS.yaml" "$CASE_DIR/ARCHITECTURE-SPINE.md" "$WORK/repo/"
cd "$WORK/repo"
git init -q
git config user.email e2e@example.invalid
git config user.name "e2e"
git add -A
git commit -qm "baseline: спеки, модель, правила"
BASE="$(git rev-parse HEAD)"

say() { printf '\n== %s ==\n' "$*"; }

# Проверка отчёта гейта: $1 — json-файл, $2 — ожидание (pass|fail),
# $3 — строка, обязанная быть в detail составляющей delta_guard (или "-").
check_gate() {
  python3 - "$1" "$2" "$3" <<'PY'
import json, sys
path, want, needle = sys.argv[1], sys.argv[2], sys.argv[3]
v = json.load(open(path, encoding="utf-8"))
dg = next(c for c in v["components"] if c["name"] == "delta_guard")
passed = v["verdict"] == "PASS" and v["exit_code"] == 0
status = dg["status"]
detail = dg.get("detail", "")
if want == "pass":
    assert passed and status == "PASS", f"ожидался PASS: {json.dumps(dg, ensure_ascii=False)}"
else:
    assert not passed and status == "FAIL", f"ожидался FAIL: {json.dumps(dg, ensure_ascii=False)}"
if needle != "-":
    blob = detail + json.dumps(dg.get("findings", []), ensure_ascii=False)
    assert needle in blob, f"в delta_guard нет '{needle}': {blob}"
print(f"гейт: {v['verdict']}, delta_guard {status}: {detail[:200]}")
PY
}

say "Шаг 0. Чистое дерево: gate PASS"
"$ARCH_BE" gate --repo . --route fast --base HEAD --format json > "$WORK/g0.json"
check_gate "$WORK/g0.json" pass "-"

say "Шаг 1. Правка model/CMP-001 в рамках активного change add-limits"
python3 - <<'PY'
import pathlib
p = pathlib.Path("model/CMP-001-priyom-platezhey.md")
text = p.read_text(encoding="utf-8")
assert "depends_on" not in text, "ребро уже есть — кейс изменился?"
p.write_text(
    text.replace(
        "code_roots: [src/intake]",
        "code_roots: [src/intake]\ndepends_on: [CMP-002]",
        1,
    ),
    encoding="utf-8",
)
PY
git add -A && git commit -qm "model: CMP-001 получает depends_on CMP-002 (change add-limits)"
"$ARCH_BE" gate --repo . --route fast --base "$BASE" --format json > "$WORK/g1.json"
check_gate "$WORK/g1.json" pass "openspec:add-limits"

say "Шаг 2. Контроль: правка CONSTRAINTS.yaml НЕ упомянута в change → FAIL"
printf '\n# правка мимо change\n' >> CONSTRAINTS.yaml
git add -A && git commit -qm "constraints: правка мимо change (контроль)"
set +e
# Текстовый вывод: пути нарушений печатаются в находках (в JSON — свёртка).
"$ARCH_BE" gate --repo . --route fast --base HEAD~1 > "$WORK/g2.txt" 2>&1
rc=$?
set -e
if [ "$rc" -eq 0 ]; then
  echo "ОШИБКА: гейт пропустил правку защищённого файла мимо change" >&2
  exit 1
fi
grep -q "CONSTRAINTS.yaml — не упоминается" "$WORK/g2.txt" || { echo "ОШИБКА: находка не называет CONSTRAINTS.yaml" >&2; exit 1; }
echo "OK: гейт красный (exit $rc), находка delta_guard называет CONSTRAINTS.yaml:"
grep -m1 "не упоминается" "$WORK/g2.txt" | sed 's/^/    /'
git reset --hard -q HEAD~1   # контрольная правка отброшена

say "Шаг 3. openspec gate --archive add-limits (перед архивацией)"
"$ARCH_BE" openspec gate --archive . add-limits

say "Шаг 4. Архивирование change (семантика openspec archive)"
mkdir -p openspec/changes/archive
git mv openspec/changes/add-limits openspec/changes/archive/2026-10-08-add-limits
# Требование дельты сливается в живую спеку capability.
python3 - <<'PY'
import pathlib
delta = pathlib.Path("openspec/changes/archive/2026-10-08-add-limits/specs/payments/spec.md").read_text(encoding="utf-8")
req = delta.split("## ADDED Requirements", 1)[1].strip()
spec = pathlib.Path("openspec/specs/payments/spec.md")
spec.write_text(spec.read_text(encoding="utf-8").rstrip() + "\n\n" + req + "\n", encoding="utf-8")
PY
git add -A && git commit -qm "archive: add-limits реализован и заархивирован"

say "Шаг 5. Gate на диапазоне, включающем архивацию (BASE..HEAD)"
"$ARCH_BE" gate --repo . --route fast --base "$BASE" --format json > "$WORK/g5.json"
check_gate "$WORK/g5.json" pass "openspec:2026-10-08-add-limits"

say "Шаг 6. Покрытие требований после слияния дельты в живую спеку"
"$ARCH_BE" openspec coverage . --strict

printf '\nИтог: e2e openspec-bridge PASS\n'
