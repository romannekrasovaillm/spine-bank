#!/usr/bin/env python3
"""termshot.py — рендер терминальной сессии из текстового сценария в SVG/PNG.

Стиль повторяет кадры README Spine (Tokyo Night, оконный хром, DejaVu Sans Mono):
те же метрики ячеек, что у src/tui/shot.rs (15px шрифт, строка 19px, cell 9.03px).

Формат сценария (UTF-8, построчно):
  Первая строка без префикса  → заголовок окна
  $ <команда>                 → промпт (зелёный '$' + яркая команда)
  # <текст>                   → комментарий (приглушённый синий)
  ! <текст>                   → акцент (жёлтый; вердикты, важные строки)
  + <текст>                   → успех (зелёный)
  - <текст>                   → ошибка/блок (красный)
  любая другая строка         → обычный вывод (светло-серый)

Использование:
  termshot.py session.txt out.svg            # только SVG
  termshot.py session.txt out.svg --png out.png  # + PNG через playwright
Переменная окружения TERMSHOT_PYTHON — интерпретатор с playwright
(по умолчанию: python3; PNG требует `pip install playwright` + chromium).
"""
import os
import subprocess
import sys

FONT = "DejaVu Sans Mono, Noto Color Emoji, monospace"
FONT_SIZE = 15.0
CELL_W = 9.03
ROW_H = 19.0
PAD_X = 14.0
CHROME_H = 26.0
PAD_BOTTOM = 12.0
PAD_TOP_TEXT = 8.0

BG = "#1a1b26"
FG = "#c0caf5"
DIM = "#565f89"
GREEN = "#9ece6a"
YELLOW = "#e0af68"
RED = "#f7768e"
BRIGHT = "#c0caf5"
BORDER = "#3b4261"

ROLES = {
    "$": ("prompt", None),
    "#": ("comment", DIM),
    "!": ("accent", YELLOW),
    "+": ("ok", GREEN),
    "-": ("err", RED),
}


def esc(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def render_lines(lines):
    """Возвращает список текстовых ролей: (роль, текст_без_префикса)."""
    out = []
    for raw in lines:
        role = "out"
        text = raw
        if raw[:2] in ("$ ", "# ", "! ", "+ ", "- "):
            role = ROLES[raw[0]][0]
            text = raw[2:]
        out.append((role, text))
    return out


def wrap_rows(rows, max_cols=118):
    """Сворачивание длинных строк: перенос с отступом-продолжением."""
    out = []
    for role, text in rows:
        t = text
        first = True
        while len(t) > max_cols:
            cut = t.rfind(" ", 0, max_cols)
            if cut < max_cols // 2:
                cut = max_cols
            chunk, t = t[:cut], t[cut:].lstrip()
            out.append((role if first else "out", chunk if first else "    " + chunk))
            first = False
        out.append((role, t if first else "    " + t))
    return out


def build_svg(title: str, rows):
    cols = max([len(t) + (2 if r == "prompt" else 0) for r, t in rows] + [len(title) + 6, 40])
    w = int(PAD_X * 2 + cols * CELL_W)
    h = int(CHROME_H + PAD_TOP_TEXT + len(rows) * ROW_H + PAD_BOTTOM)
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">',
        f'<rect x="0.5" y="0.5" width="{w-1}" height="{h-1}" rx="10" fill="{BG}" stroke="{BORDER}"/>',
        f'<circle cx="20" cy="14" r="5" fill="#f7768e"/>',
        f'<circle cx="38" cy="14" r="5" fill="#e0af68"/>',
        f'<circle cx="56" cy="14" r="5" fill="#9ece6a"/>',
        f'<text x="{w/2}" y="18" text-anchor="middle" font-family="{FONT}" font-size="12" fill="{DIM}">{esc(title)}</text>',
    ]
    y = CHROME_H + PAD_TOP_TEXT + FONT_SIZE
    for role, text in rows:
        x = PAD_X
        if role == "prompt":
            parts.append(
                f'<text x="{x}" y="{y}" font-family="{FONT}" font-size="{FONT_SIZE}" fill="{GREEN}">$</text>'
            )
            parts.append(
                f'<text x="{x + 2*CELL_W}" y="{y}" font-family="{FONT}" font-size="{FONT_SIZE}" fill="{BRIGHT}">{esc(text)}</text>'
            )
        else:
            color = {
                "comment": DIM,
                "accent": YELLOW,
                "ok": GREEN,
                "err": RED,
            }.get(role, FG)
            parts.append(
                f'<text x="{x}" y="{y}" font-family="{FONT}" font-size="{FONT_SIZE}" fill="{color}">{esc(text)}</text>'
            )
        y += ROW_H
    parts.append("</svg>")
    return "\n".join(parts)


def main():
    src, out_svg = sys.argv[1], sys.argv[2]
    png = None
    scale = "2"
    if "--png" in sys.argv:
        png = sys.argv[sys.argv.index("--png") + 1]
    if "--scale" in sys.argv:
        scale = sys.argv[sys.argv.index("--scale") + 1]
    with open(src, encoding="utf-8") as f:
        lines = f.read().splitlines()
    title = lines[0] if lines else "terminal"
    rows = wrap_rows(render_lines(lines[1:]))
    svg = build_svg(title, rows)
    with open(out_svg, "w", encoding="utf-8") as f:
        f.write(svg)
    if png:
        cols = max([len(t) + (2 if r == "prompt" else 0) for r, t in rows] + [len(title) + 6, 40])
        w = int(PAD_X * 2 + cols * CELL_W)
        h = int(CHROME_H + PAD_TOP_TEXT + len(rows) * ROW_H + PAD_BOTTOM)
        html = (
            f"<!doctype html><html><head><meta charset='utf-8'>"
            f"<style>body{{margin:0;background:{BG};}}svg{{display:block;}}</style>"
            f"</head><body>{svg}</body></html>"
        )
        html_path = out_svg + ".html"
        with open(html_path, "w", encoding="utf-8") as f:
            f.write(html)
        pw = (
            "from playwright.sync_api import sync_playwright\n"
            "with sync_playwright() as p:\n"
            "    b = p.chromium.launch()\n"
            f"    pg = b.new_page(viewport={{'width': {w}, 'height': {h}}}, device_scale_factor={scale})\n"
            f"    pg.goto('file://{html_path}')\n"
            "    pg.wait_for_timeout(300)\n"
            f"    pg.screenshot(path='{png}')\n"
            "    b.close()\n"
        )
        subprocess.run(
            [os.environ.get("TERMSHOT_PYTHON", "python3"), "-c", pw],
            check=True,
            capture_output=True,
        )
    print(f"svg={out_svg}" + (f" png={png}" if png else ""))


if __name__ == "__main__":
    main()
