#!/usr/bin/env python3
"""control_standard.py — метрики 7, 9, 10 (§9): выразимость стандарта и корп-правила.

Метрика 7 — доля требований S1–S12, механизованных ИСПОЛНЯЕМОЙ проверкой
(`type` ≠ `unverifiable`). Считается по фактическому содержимому
`standard/CONSTRAINTS.java-services.yaml`, а не по карте `requirements.yaml`
(карта — ожидание, зашитое до прогона). Порог 0.4.0: ≥ 11/12.

Метрика 9 — ложные FAIL от неприменимых корп-правил: на PetClinic (Java-сервисы,
но другие пакеты) глобы `**/application/domain/**` и `**/adapter/in/**` не находят
файлов; правильно — N/A, дефект — ERROR. Считаем ERROR-находки на пустом глоба.

Метрика 10 — форматы манифестов тех-радара: `deny_dependency` должен видеть
build.gradle, package.json, *.csproj, pyproject.toml, а не только pom.xml/
Cargo.toml/requirements.txt/go.mod. Проба: 8 манифестов, каждый объявляет
запрещённый пакет; считаем, сколько файлов дали находку.

Exit 0 (измерение); SKIP честно печатается при отсутствии входа.
"""
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

STANDARD = avlib.STANDARD / "CONSTRAINTS.java-services.yaml"

# 8 форматов манифестов (по одному файлу в подкаталоге) — восьмёрка тех-радара.
# Каждый объявляет пакет `h2`; правило deny_dependency с deny: [h2].
MANIFEST_PROBE = {
    "svc-maven/pom.xml": (
        '<project><dependencies><dependency>'
        '<groupId>com.h2database</groupId><artifactId>h2</artifactId>'
        '</dependency></dependencies></project>\n'
    ),
    "svc-gradle/build.gradle": "dependencies {\n  implementation 'com.h2database:h2:2.2.224'\n}\n",
    "svc-node/package.json": '{"name":"svc","dependencies":{"h2":"^2.0.0"}}\n',
    "svc-dotnet/svc.csproj": (
        '<Project Sdk="Microsoft.NET.Sdk"><ItemGroup>'
        '<PackageReference Include="h2" Version="2.0.0" />'
        '</ItemGroup></Project>\n'
    ),
    "svc-python/pyproject.toml": '[project]\nname = "svc"\ndependencies = ["h2"]\n',
    "svc-req/requirements.txt": "h2==2.0.0\n",
    "svc-rust/Cargo.toml": '[package]\nname = "svc"\nversion = "0.1.0"\n\n[dependencies]\nh2 = "2.0"\n',
    "svc-go/go.mod": "module example.com/svc\n\ngo 1.21\n\nrequire github.com/example/h2 v1.0.0\n",
}

PROBE_CONSTRAINTS = """version: "2026.10"
rules:
  - id: PROBE-010
    name: probe_manifest_formats
    type: deny_dependency
    manifests: ["**/pom.xml", "**/build.gradle", "**/build.gradle.kts",
                "**/package.json", "**/*.csproj", "**/pyproject.toml",
                "**/requirements.txt", "**/Cargo.toml", "**/go.mod"]
    deny: [h2]
    severity: error
    owner: "@bench"
    rationale: "Проба форматов манифестов тех-радара (метрика 10)."
"""


def control_check_json(repo, constraints):
    r = avlib.run_archbe(
        ["control", "check", str(repo), "--constraints", str(constraints), "--json"],
    )
    if r.code not in (0, 1):  # 1 = есть error-находки (нормальный исход)
        return None, f"control check → код {r.code}: {(r.err or '').strip()[:160]}"
    try:
        return json.loads(r.out), None
    except json.JSONDecodeError as e:
        return None, f"control check: не JSON ({e})"


def metric7():
    std = avlib.load_yaml(STANDARD)
    rules = std.get("rules") if isinstance(std, dict) else None
    if not isinstance(rules, list):
        avlib.emit_skip("metric 7", f"не разобран {STANDARD}")
        return
    executable = [r for r in rules if r.get("type") and not r.get("unverifiable")]
    total = len(rules)
    avlib.emit_run(
        "metric 7",
        f"expressible={len(executable)}/{total} "
        f"(baseline=7, target≥11; ids={','.join(str(r.get('id')) for r in executable)})",
    )


def metric9(repos_root):
    repo = avlib.repo_clone_dir("petclinic", repos_root)
    if repo is None:
        avlib.emit_skip("metric 9", "клон petclinic отсутствует")
        return
    doc, err = control_check_json(repo, STANDARD)
    if err:
        avlib.emit_skip("metric 9", err)
        return
    issues = doc.get("issues", [])
    errors = [i for i in issues if i.get("severity") == "error"]
    empty_glob = [i for i in errors if "не найдено ни одного файла" in (i.get("message") or "")]
    avlib.emit_run(
        "metric 9",
        f"false_fail_errors={len(errors)} (из них пустой glob: {len(empty_glob)}; "
        f"baseline=2, target=0) rules={','.join(sorted({i.get('rule', '') for i in errors}))}",
    )


def metric10():
    with tempfile.TemporaryDirectory(prefix="manifest_probe_") as td:
        root = Path(td)
        for rel, content in MANIFEST_PROBE.items():
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(content, encoding="utf-8")
        constraints = root / "CONSTRAINTS.probe.yaml"
        constraints.write_text(PROBE_CONSTRAINTS, encoding="utf-8")
        doc, err = control_check_json(root, constraints)
        if err:
            avlib.emit_skip("metric 10", err)
            return
        hits = {
            i.get("file", "")
            for i in doc.get("issues", [])
            if i.get("rule") == "probe_manifest_formats"
        }
        # сопоставляем находку с форматом по имени файла
        seen = {rel for rel in MANIFEST_PROBE if any(h.endswith(rel) for h in hits)}
        missed = sorted(set(MANIFEST_PROBE) - seen)
        avlib.emit_run(
            "metric 10",
            f"formats_seen={len(seen)}/8 (baseline=4/8, target=8/8 + диапазоны/scope)"
            + (f"; missed={','.join(m.split('/')[-1] for m in missed)}" if missed else ""),
        )


def main(repos_root=None):
    avlib.emit_section("Стандарт Java-сервисы — метрики 7, 9, 10")
    metric7()
    metric9(repos_root)
    metric10()
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
