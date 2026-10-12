#!/usr/bin/env python3
"""
change_risk_scan.py — предлагает уровень риска (T0–T3) для изменения по git diff.

Только стандартная библиотека Python 3.8+. Результат — ПРЕДЛОЖЕНИЕ для архитектора,
а не вердикт: уровень повышается правилами путей и эвристиками, итог = максимум.

Примеры:
  python change_risk_scan.py --base origin/main --head HEAD
  python change_risk_scan.py --base main --config risk-config.json --format json
  python change_risk_scan.py --staged            # проверить индекс перед коммитом

Коды выхода: 0 — успех; 1 — ошибка git/конфига;
с флагом --fail-at N возвращает 3, если итоговый уровень >= N (для CI).
"""
import argparse
import fnmatch
import json
import subprocess
import sys
from pathlib import Path

DEFAULT_CONFIG = {
    "tiers": {
        "0": "T0 Рутина: автоприёмка при зелёных проверках",
        "1": "T1 Локальное: AI-ревью + выборочная проверка командой",
        "2": "T2 Структурное: асинхронное ревью архитектора на уровне design/delta",
        "3": "T3 Критическое: синхронное ревью, ADR или запрос на решение",
    },
    "rules": [
        {"id": "data-migration", "tier": 3, "reason": "Миграции схем данных",
         "globs": ["**/migrations/**", "**/*.sql", "**/alembic/versions/**"]},
        {"id": "authn-authz", "tier": 3, "reason": "Аутентификация, авторизация, секреты",
         "globs": ["**/auth/**", "**/security/**", "**/*secret*", "**/iam/**"]},
        {"id": "dependencies", "tier": 2, "reason": "Изменение зависимостей",
         "globs": ["**/package.json", "**/pom.xml", "**/build.gradle*", "**/go.mod",
                   "**/requirements*.txt", "**/pyproject.toml", "**/Cargo.toml", "**/*.csproj"]},
        {"id": "infrastructure", "tier": 2, "reason": "IaC и развёртывание",
         "globs": ["**/*.tf", "**/helm/**", "**/k8s/**", "**/Dockerfile"]},
        {"id": "ci-cd", "tier": 2, "reason": "Пайплайны CI/CD",
         "globs": [".github/workflows/**", ".gitlab-ci.yml", "Jenkinsfile"]},
        {"id": "arch-checks", "tier": 2, "reason": "Архитектурные проверки и их исключения",
         "globs": ["**/.dependency-cruiser*", "**/.importlinter", "**/.semgrep/**",
                   "**/arch-baseline*.json", ".claude/settings*.json", ".claude/hooks/**"]},
        {"id": "agent-context", "tier": 1, "reason": "Контекст агентов и решения",
         "globs": ["**/CLAUDE.md", "**/AGENTS.md", "docs/adr/**", "openspec/specs/**"]},
    ],
    "size": {"t1_changed_lines": 300, "t2_changed_lines": 1500},
    "test_weakening": {"enabled": True, "tier": 2,
                       "test_globs": ["**/test/**", "**/tests/**", "**/*_test.*",
                                      "**/*.test.*", "**/*.spec.*", "**/test_*.py"]},
}


def run_git(args):
    try:
        out = subprocess.run(["git"] + args, check=True, capture_output=True, text=True)
        return out.stdout
    except FileNotFoundError:
        sys.exit("Ошибка: git не найден в PATH.")
    except subprocess.CalledProcessError as e:
        sys.stderr.write(e.stderr)
        sys.exit(1)


def match(path, pattern):
    """fnmatch, где '**/' в начале также допускает совпадение в корне репозитория."""
    if fnmatch.fnmatch(path, pattern):
        return True
    if pattern.startswith("**/") and fnmatch.fnmatch(path, pattern[3:]):
        return True
    return False


def collect_changes(base, head, staged):
    if staged:
        rng = ["--cached"]
    else:
        rng = [f"{base}...{head}"]
    numstat = run_git(["diff", "--numstat", "-M"] + rng)
    status = run_git(["diff", "--name-status", "-M"] + rng)

    files = {}
    for line in numstat.splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        added, deleted, path = parts[0], parts[1], parts[-1]
        if " => " in path:  # переименование в формате numstat
            path = path.split(" => ")[-1].replace("}", "").replace("{", "")
        files[path] = {
            "added": int(added) if added.isdigit() else 0,
            "deleted": int(deleted) if deleted.isdigit() else 0,
            "binary": added == "-",
            "status": "M",
        }
    for line in status.splitlines():
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        st, path = parts[0][0], parts[-1]
        files.setdefault(path, {"added": 0, "deleted": 0, "binary": False})
        files[path]["status"] = st
    return files


def classify(files, config):
    findings = []
    tier = 0

    for rule in config.get("rules", []):
        hit = sorted(p for p in files if any(match(p, g) for g in rule.get("globs", [])))
        if hit:
            findings.append({"id": rule["id"], "tier": int(rule["tier"]),
                             "reason": rule.get("reason", ""), "files": hit})
            tier = max(tier, int(rule["tier"]))

    total = sum(f["added"] + f["deleted"] for f in files.values())
    size = config.get("size", {})
    size_tier = 0
    if total >= size.get("t2_changed_lines", 10**9):
        size_tier = 2
    elif total >= size.get("t1_changed_lines", 10**9):
        size_tier = 1
    if size_tier:
        findings.append({"id": "size", "tier": size_tier,
                         "reason": f"Объём изменения: {total} строк (размер сам по себе не делает изменение T3)",
                         "files": []})
        tier = max(tier, size_tier)

    tw = config.get("test_weakening", {})
    if tw.get("enabled"):
        tests = {p: f for p, f in files.items() if any(match(p, g) for g in tw.get("test_globs", []))}
        removed = sorted(p for p, f in tests.items() if f.get("status") == "D")
        shrunk = sorted(p for p, f in tests.items()
                        if f.get("status") != "D" and f["deleted"] > f["added"] and f["deleted"] >= 10)
        if removed or shrunk:
            findings.append({"id": "test-weakening", "tier": int(tw.get("tier", 2)),
                             "reason": "Тесты удалены или заметно сокращены — проверь, не ослаблены ли проверки",
                             "files": removed + shrunk})
            tier = max(tier, int(tw.get("tier", 2)))

    if files and tier == 0:
        findings.append({"id": "default", "tier": 0,
                         "reason": "Совпадений с рискованными правилами нет", "files": []})
    return tier, total, findings


def render_md(tier, total, files, findings, config):
    tiers = config.get("tiers", DEFAULT_CONFIG["tiers"])
    lines = [
        "# Предварительная классификация риска изменения",
        "",
        f"**Предложенный уровень: T{tier}** — {tiers.get(str(tier), '')}",
        f"Файлов: {len(files)}, изменённых строк: {total}",
        "",
        "> Это предложение скрипта. Архитектор подтверждает или корректирует уровень и объясняет почему.",
        "",
        "| Правило | Уровень | Причина | Файлы |",
        "|---|---|---|---|",
    ]
    for f in sorted(findings, key=lambda x: -x["tier"]):
        shown = ", ".join(f"`{p}`" for p in f["files"][:5])
        if len(f["files"]) > 5:
            shown += f" и ещё {len(f['files']) - 5}"
        lines.append(f"| {f['id']} | T{f['tier']} | {f['reason']} | {shown or '—'} |")
    lines += ["", "## Что проверить дальше"]
    if tier >= 2:
        lines.append("- Начни с намерения и дизайна (proposal/design/plan/story), затем дельта по чек-листу.")
        lines.append("- Собери неявные решения (новые зависимости, контракты, данные).")
    if tier >= 3:
        lines.append("- Нужен ADR или запрос на решение и именованный одобряющий.")
    if any(f["id"] == "test-weakening" for f in findings):
        lines.append("- Убедись, что удалённые или сокращённые тесты не ослабили проверку поведения.")
    if tier <= 1:
        lines.append("- Достаточно автоматических проверок и AI-ревью согласно политике.")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description="Предложить уровень риска изменения по git diff.")
    ap.add_argument("--base", default="origin/main", help="базовая ревизия (по умолчанию origin/main)")
    ap.add_argument("--head", default="HEAD", help="целевая ревизия (по умолчанию HEAD)")
    ap.add_argument("--staged", action="store_true", help="анализировать индекс (для pre-commit)")
    ap.add_argument("--config", help="JSON-конфиг правил (см. assets/risk-config.example.json)")
    ap.add_argument("--format", choices=["md", "json"], default="md")
    ap.add_argument("--fail-at", type=int, choices=[1, 2, 3],
                    help="вернуть код 3, если уровень >= значения (для CI-маршрутизации)")
    args = ap.parse_args()

    config = DEFAULT_CONFIG
    if args.config:
        try:
            config = json.loads(Path(args.config).read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as e:
            sys.exit(f"Ошибка чтения конфига: {e}")

    files = collect_changes(args.base, args.head, args.staged)
    if not files:
        print("Изменений не найдено для заданного диапазона." if args.format == "md"
              else json.dumps({"tier": 0, "changed_lines": 0, "files": 0, "findings": []}))
        return
    tier, total, findings = classify(files, config)

    if args.format == "json":
        print(json.dumps({"tier": tier, "changed_lines": total, "files": len(files),
                          "findings": findings}, ensure_ascii=False, indent=2))
    else:
        print(render_md(tier, total, files, findings, config))

    if args.fail_at is not None and tier >= args.fail_at:
        sys.exit(3)


if __name__ == "__main__":
    main()
