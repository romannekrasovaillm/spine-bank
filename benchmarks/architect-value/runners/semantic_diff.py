#!/usr/bin/env python3
"""semantic_diff.py — семантические зонды контрактов (AB-12, ADR-079).

Прогоняет `arch-be contract-diff <old> <new> --json --semantic` на пяти
фикстурах `fixtures/semantic/`: четыре обязаны дать семантический сигнал
(enum/единица измерения/`default`/ключ партиционирования), пятая (benign) —
НЕ дать ни одного (защита от ложных срабатываний). Самопроверка бенчмарка:
4 сигнала + 1 чистый.

Матрица (правило → severity → ожидаемый `passed`):
  enum_remove   → semantic_enum_narrowed       (error) → passed=false
  unit_rename   → semantic_unit_changed         (error) → passed=false
  default_shift → semantic_default_shift        (warn)  → passed=true
  partition_key → semantic_partition_key_changed (warn) → passed=true
  benign        → (semantic_changes пуст)       —       → passed=true

Дополнительно: без флага `--semantic` JSON НЕ содержит ключа
`semantic_changes` (обратная совместимость `contract-diff/v1`).

Exit 0 — всегда кроме неработающего arch-be; SKIP ≠ PASS.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

FIXTURES = avlib.BASE / "fixtures" / "semantic"

# (имя фикстуры, ожидаемое правило, severity, ожидаемый passed). None-правило —
# benign: семантический список обязан быть пуст.
MATRIX = [
    ("enum_remove", "semantic_enum_narrowed", "error", False),
    ("unit_rename", "semantic_unit_changed", "error", False),
    ("default_shift", "semantic_default_shift", "warn", True),
    ("partition_key", "semantic_partition_key_changed", "warn", True),
    ("benign", None, None, True),
]


def run_fixture(name, semantics):
    """Прогон одной фикстуры → (doc|None, argv-строка, причина)."""
    old = FIXTURES / f"{name}.old.yaml"
    new = FIXTURES / f"{name}.new.yaml"
    if not old.is_file() or not new.is_file():
        return None, f"{old} / {new}", f"фикстура отсутствует ({FIXTURES})"
    args = ["contract-diff", str(old), str(new), "--json"]
    if semantics:
        args.append("--semantic")
    r = avlib.run_archbe(args, cwd=avlib.BASE)
    # `contract-diff` использует код возврата как гейт: 1 = есть breaking
    # (passed=false), 0 = чисто. Это НЕ сбой инструмента — JSON в stdout валиден
    # при любом коде, поэтому разбираем stdout, а код проверяем как fallback.
    try:
        return json.loads(r.out), " ".join(args), None
    except json.JSONDecodeError as e:
        return None, " ".join(args), f"arch-be код {r.code}, stdout не JSON ({e}): {(r.err or '')[:160]}"


def check_fixture(name, rule, severity, expect_passed):
    """Проверка одной фикстуры с флагом → True, если сигнал совпал."""
    doc, argv, err = run_fixture(name, semantics=True)
    if err:
        avlib.emit_skip(f"semantic [{name}]", err)
        return False
    sem = doc.get("semantic_changes") or []
    passed = bool(doc.get("passed"))
    if rule is None:
        ok = not sem and passed == expect_passed
        detail = f"semantic_changes={len(sem)} (ожидалось 0), passed={passed}"
    else:
        hit = [f for f in sem if f.get("rule") == rule]
        ok = (
            len(hit) == 1
            and hit[0].get("severity") == severity
            and passed == expect_passed
        )
        got = ", ".join(f"{f.get('rule')}/{f.get('severity')}" for f in sem) or "нет"
        detail = f"получено [{got}], passed={passed}"
    if ok:
        what = "чисто (FP нет)" if rule is None else f"{rule} ({severity})"
        avlib.emit_run(
            f"semantic [{name}]", f"{what}; passed={passed} — как ожидалось"
        )
    else:
        want = (
            "semantic_changes пуст"
            if rule is None
            else f"{rule} ({severity}), passed={expect_passed}"
        )
        avlib.emit_find(
            f"semantic [{name}]", f"ожидалось {want}, {detail} (cmd: {argv})"
        )
    return ok


def check_backward_compat():
    """Без `--semantic` ключ `semantic_changes` в JSON отсутствует."""
    doc, argv, err = run_fixture("enum_remove", semantics=False)
    if err:
        avlib.emit_skip("semantic [без флага]", err)
        return False
    leaked = "semantic_changes" in doc
    if leaked:
        avlib.emit_find(
            "semantic [без флага]",
            f"ключ semantic_changes просочился без флага (cmd: {argv})",
        )
        return False
    avlib.emit_run(
        "semantic [без флага]",
        "JSON без флага не содержит semantic_changes — contract-diff/v1 не изменён",
    )
    return True


def main(_repos_root=None):
    avlib.emit_section("CONTRACT — семантические зонды контрактов (AB-12, ADR-079)")
    if not FIXTURES.is_dir():
        avlib.emit_skip("semantic", f"нет каталога фикстур {FIXTURES}")
        return 0
    signals = 0
    for name, rule, severity, passed in MATRIX:
        if check_fixture(name, rule, severity, passed):
            signals += 1
    compat = check_backward_compat()
    total = len(MATRIX)
    expected = total  # 4 сигнала + 1 чистый — все пять обязаны совпасть
    ok = signals == expected and compat
    avlib.emit_run(
        "semantic (итог)",
        f"совпало {signals}/{total} фикстур (4 сигнала + 1 чистый, ожидалось {expected}), "
        f"обратная совместимость {'OK' if compat else 'FAIL'}"
        f"{' OK' if ok else ' BELOW'}",
    )
    if not ok:
        avlib.emit_find(
            "semantic",
            "не все семантические зонды подтвердились на фикстурах — "
            "проверить semantic_changes и отсутствие ключа без флага",
        )
    return 0


if __name__ == "__main__":
    _root = None
    _args = sys.argv[1:]
    if _args and _args[0] == "--repos-dir" and len(_args) > 1:
        _root = _args[1]
    sys.exit(main(_root))
