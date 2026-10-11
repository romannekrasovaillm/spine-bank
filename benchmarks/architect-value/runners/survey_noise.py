#!/usr/bin/env python3
"""survey_noise.py — метрика 6 (§9): доля шумовых строк в карте обследования.

Волна B (B8-денилист в `src/survey.rs`). Прогоняет `arch-be survey <boutique>`
в отдельный каталог (`--out <tmp>`, клон не загрязняется — как в
`survey_compare.py`), разбирает `survey.md` и считает долю строк-шума.

Шум (по смыслу baseline §9 «12 интеграций-шум + 40 engines»):
  • строка-находка ссылается на ХОСТ из встроенного денилиста B8
    (домены лицензий/funding — не интеграции);
  • строка-находка ссылается на ПУТЬ из встроенного денилиста B8 (lock-файлы,
    метаданные чартов — не источники endpoint'ов и версий платформы);
  • строка содержит поле `engines` (метаданные пакетного менеджера).
Денилисты продублированы из `src/survey.rs` (`BUILTIN_DENY_HOSTS` /
`BUILTIN_DENY_PATHS`) — прогонщик сверяет ФАКТ выхода, а не пересчитывает B8.

Порог §9: ≤ 5 % строк (baseline ≥ 52 шумовых строки). Нет клона → SKIP.
exit 0 (измерение).

Предикат применяется ко ВСЕЙ карте (не только к §«Интеграции»): остаточный
шум — упоминания lock-файлов в секции churn (§«Риски изменения»), они честно
печатаются и остаются под порогом. Если денилист деградирует и lock-файлы
полезут в §«Интеграции», доля выйдет за 5 % — метрика сработает.
"""
import re
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

REPO = "boutique"
THRESHOLD = 0.05

# Дубли `src/survey.rs` (B8 волны B) — источник истины остаётся в движке.
DENY_HOSTS = {
    "apache.org", "opensource.org", "gnu.org", "creativecommons.org",
    "mit-license.org", "mozilla.org", "semver.org", "paypal.me", "patreon.com",
    "opencollective.com", "ko-fi.com", "buymeacoffee.com", "liberapay.com",
}
DENY_PATHS = {
    "package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml",
    "cargo.lock", "gemfile.lock", "composer.lock", "poetry.lock", "uv.lock",
    "packages.lock.json", "chart.yaml", "chart.lock", "flake.lock",
}

BULLET = re.compile(r"^\s*-\s*\[(?:confirmed|gap)\]")
EVIDENCE = re.compile(r"\(([^()]*)\)\s*$")
HOST_TOKEN = re.compile(r"\b([a-z][a-z0-9._-]*\.[a-z]{2,})\b", re.IGNORECASE)


def path_denied(path):
    """Имя файла из денилиста B8 (по равенству basename или суффиксу `*.lock`)."""
    base = path.strip().split(":", 1)[0].rsplit("/", 1)[-1].lower()
    if base in DENY_PATHS:
        return True
    return base.endswith(".lock")


def host_denied(host):
    """Хост из денилиста B8 (равенство или dot-суффикс, как `host_ignored`)."""
    h = host.lower().rstrip(".")
    return any(h == d or h.endswith("." + d) for d in DENY_HOSTS)


def line_is_noise(line):
    """Шумовая ли строка-находка: denied-хост / denied-путь / `engines`."""
    if re.search(r"\bengines\b", line):
        return True
    m = EVIDENCE.search(line)
    if m:
        for chunk in m.group(1).split(","):
            chunk = chunk.strip()
            if chunk and path_denied(chunk):
                return True
    for tok in HOST_TOKEN.findall(line):
        if host_denied(tok):
            return True
    return False


def analyze(text):
    """(total_bullets, noise_lines[]) по содержимому survey.md."""
    bullets, noise = 0, []
    for raw in text.splitlines():
        if not BULLET.match(raw):
            continue
        bullets += 1
        if line_is_noise(raw):
            noise.append(raw.strip())
    return bullets, noise


def main(repos_root=None):
    avlib.emit_section("ArchRecover — метрика 6 (шум карты обследования)")
    clone = avlib.repo_clone_dir(REPO, repos_root)
    if clone is None:
        avlib.emit_skip("metric 6", "клон boutique отсутствует (--repos-dir/ARCH_VALUE_REPOS)")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="survey-noise-"))
    try:
        r = avlib.run_archbe(["survey", str(clone), "--out", str(tmp)], cwd=clone)
        if not r.ok:
            avlib.emit_skip("metric 6", f"survey → код {r.code}: {(r.err or '').strip()[:160]}")
            return 0
        md = tmp / "survey.md"
        if not md.is_file():
            avlib.emit_skip("metric 6", "survey не создал survey.md")
            return 0
        bullets, noise = analyze(md.read_text(encoding="utf-8"))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    if bullets == 0:
        avlib.emit_skip("metric 6", "в карте нет строк-находок — измерить долю шума нечего")
        return 0
    ratio = len(noise) / bullets
    ok = ratio <= THRESHOLD
    avlib.emit_run(
        f"metric 6 [{REPO}]",
        f"шум {len(noise)}/{bullets} ({ratio:.3f})  {'OK' if ok else 'ABOVE'} (порог ≤{THRESHOLD:.2f})",
    )
    for line in noise:
        avlib.emit_run(f"metric 6 [{REPO}] шумовая строка", line)
    if not ok:
        avlib.emit_find(
            "metric 6",
            "доля шумовых строк превышает порог: B8-денилист не отсекает "
            "lock/лицензии/engines — проверить `BUILTIN_DENY_*` в `src/survey.rs`.",
        )
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
