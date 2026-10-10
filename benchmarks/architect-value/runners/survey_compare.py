#!/usr/bin/env python3
"""survey_compare.py — метрика 1 (§9): `survey .` ≡ `survey <abs>`.

Для каждого репо B с доступным клоном прогоняет карту обследования ДВАЖДЫ —
относительным путём (cwd = клон) и абсолютным — и сравнивает НАБОР находок
(строки вида `- [confirmed] ... (файл:строка)` / `- [gap] ...`), игнорируя
шапку (`repo:` во frontmatter и заголовок `# Карта обследования: <repo>`),
которые законно различаются формой пути.

Порог §9: наборы равны. Клонов нет → SKIP (не «зелень»). Прогонщик измеряет,
exit 0; ненулевой exit — только если не удалось запустить бинарь.

Обоснование выбора полей: `survey` пишет `survey.md` (frontmatter + находки) и
`survey-notes.md` (человеческие дополнения) — сравниваем ТОЛЬКО находки
`survey.md`, т.к. именно они суть «карта».
"""
import re
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

FIND_RE = re.compile(r"^\s*-\s*\[(?:confirmed|gap)\]\s*(.*)$")


def extract_findings(survey_md_text):
    """Множество нормализованных строк-находок `- [confirmed]/[gap] ...`."""
    out = []
    for line in survey_md_text.splitlines():
        m = FIND_RE.match(line)
        if m:
            out.append(" ".join(m.group(1).split()))
    return out


def survey_findings(repo_expr, out_dir, cwd):
    r = avlib.run_archbe(["survey", repo_expr, "--out", str(out_dir)], cwd=cwd)
    if not r.ok:
        return None, f"survey {repo_expr} → код {r.code}: {(r.err or '').strip()[:160]}"
    md = out_dir / "survey.md"
    if not md.is_file():
        return None, f"survey {repo_expr} не создал {md}"
    return extract_findings(md.read_text(encoding="utf-8")), None


def main(repos_root=None):
    avlib.emit_section("Архитектурный обход — метрика 1 (survey . ≡ survey <abs>)")
    measured = 0
    for repo in ("boutique", "petclinic", "buckpal", "library"):
        clone = avlib.repo_clone_dir(repo, repos_root)
        if clone is None:
            avlib.emit_skip(f"metric 1 [{repo}]", "клон отсутствует (--repos-dir/ARCH_VALUE_REPOS)")
            continue
        with tempfile.TemporaryDirectory(prefix=f"survey_{repo}_") as td:
            base = Path(td)
            rel, err_rel = survey_findings(".", base / "rel", cwd=clone)
            abs_, err_abs = survey_findings(str(clone), base / "abs", cwd=clone)
        if err_rel or err_abs:
            avlib.emit_skip(f"metric 1 [{repo}]", err_rel or err_abs)
            continue
        set_rel, set_abs = set(rel), set(abs_)
        equal = set_rel == set_abs
        measured += 1
        detail = f"rel={len(rel)} abs={len(abs_)} equal={'yes' if equal else 'NO'}"
        avlib.emit_run(f"metric 1 [{repo}]", detail)
        if not equal:
            only_rel = sorted(set_rel - set_abs)
            only_abs = sorted(set_abs - set_rel)
            for s in only_rel[:5]:
                avlib.emit_run(f"metric 1 [{repo}] only-rel", s[:120])
            for s in only_abs[:5]:
                avlib.emit_run(f"metric 1 [{repo}] only-abs", s[:120])
    if measured == 0:
        avlib.emit_skip("metric 1", "ни одного клона — измерить нечего")
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
