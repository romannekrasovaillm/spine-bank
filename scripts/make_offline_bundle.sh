#!/usr/bin/env bash
# make_offline_bundle.sh — офлайн-бандл Spine для закрытого контура (без
# интернета): dist/spine-offline-<version>-<os>-<arch>.tar.gz
#
# Состав архива (корень spine-offline-<version>-<os>-<arch>/):
#   bin/arch-be        бинарь (по умолчанию — core-редакция: без сети и TUI);
#   vendor/archify/    вендоренный движок диаграмм (со своим SHA256SUMS, BE-22);
#   install.sh         установка: бинарь → ~/.local/bin, vendor → ~/.arch-harness,
#                      `arch-be init` (ассеты и скиллы встроены в бинарь
#                      include_str! — интернет не нужен), cli_path для archify;
#   README.md          три шага;
#   SHA256SUMS         хэши всех файлов бандла (проверяется install.sh).
# Рядом с архивом обновляется dist/SHA256SUMS (хэш самого tar.gz).
# Naming — по релизной матрице (.github/workflows/release.yml):
# arch-be[-core]-<os>-<arch>; бандл добавляет префикс spine-offline-<version>.
#
# Использование:
#   scripts/make_offline_bundle.sh [--edition core|full] [--binary PATH] [--out DIR]
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EDITION="core"
BINARY=""
OUT_DIR="$REPO_ROOT/dist"

while [ $# -gt 0 ]; do
    case "$1" in
        --edition)
            EDITION="${2:?--edition core|full}"
            shift 2
            ;;
        --binary)
            BINARY="${2:?--binary PATH}"
            shift 2
            ;;
        --out)
            OUT_DIR="${2:?--out DIR}"
            shift 2
            ;;
        -h | --help)
            grep '^#' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "неизвестный аргумент: $1 (см. --help)" >&2
            exit 2
            ;;
    esac
done

case "$EDITION" in
    core | full) ;;
    *)
        echo "--edition: ожидалось core|full, получено '$EDITION'" >&2
        exit 2
        ;;
esac

VERSION="$(grep -m1 '^version = ' "$REPO_ROOT/Cargo.toml" | cut -d'"' -f2)"
[ -n "$VERSION" ] || {
    echo "не удалось прочитать version из Cargo.toml" >&2
    exit 1
}

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
[ "$OS" = "darwin" ] && OS="macos"
ARCH="$(uname -m)"

BUNDLE_NAME="spine-offline-$VERSION-$OS-$ARCH"
echo "== Бандл: $BUNDLE_NAME (редакция $EDITION) =="

# 1. Бинарь: готовый путь или сборка release нужной редакции.
if [ -z "$BINARY" ]; then
    if [ "$EDITION" = "core" ]; then
        (cd "$REPO_ROOT" && cargo build --release --locked --no-default-features --features core)
    else
        (cd "$REPO_ROOT" && cargo build --release --locked)
    fi
    BINARY="$REPO_ROOT/target/release/arch-be"
fi
[ -x "$BINARY" ] || {
    echo "бинарь не найден/не исполняем: $BINARY" >&2
    exit 1
}

# 2. Сцена.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ROOT="$STAGE/$BUNDLE_NAME"
mkdir -p "$ROOT/bin" "$ROOT/vendor"

install -m 0755 "$BINARY" "$ROOT/bin/arch-be"
cp -R "$REPO_ROOT/vendor/archify" "$ROOT/vendor/archify"

cat > "$ROOT/README.md" <<'README'
# Spine offline bundle (arch-be)

1. Распаковать: `tar xzf spine-offline-*.tar.gz && cd spine-offline-*`
2. Установить: `./install.sh` (бинарь → ~/.local/bin, движок Archify → ~/.arch-harness/vendor, `arch-be init` — ассеты встроены в бинарь, интернет не нужен)
3. Проверить: `arch-be doctor`; подключение хоста: `arch-be connect gigacode` в корне проекта → `arch-be doctor --host gigacode`
README

cat > "$ROOT/install.sh" <<'INSTALL'
#!/usr/bin/env bash
# Установка офлайн-бандла Spine (arch-be). Интернет не требуется.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"

echo "== Spine offline bundle: установка =="

# 0. Целостность файлов бандла.
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c SHA256SUMS --quiet
    echo "✓ целостность подтверждена (SHA256SUMS)"
else
    echo "⚠ sha256sum не найден — проверка целостности пропущена" >&2
fi

# 1. Бинарь.
BIN_DIR="${ARCH_BE_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$BIN_DIR"
install -m 0755 bin/arch-be "$BIN_DIR/arch-be"
echo "✓ бинарь: $BIN_DIR/arch-be"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "⚠ $BIN_DIR не в PATH — добавьте: export PATH=\"$BIN_DIR:\$PATH\"" >&2 ;;
esac

# 2. Вендоренный движок диаграмм Archify (нужен инструментам archify_*).
ARCH_HOME_DIR="${ARCH_HOME:-$HOME/.arch-harness}"
mkdir -p "$ARCH_HOME_DIR/vendor"
rm -rf "$ARCH_HOME_DIR/vendor/archify"
cp -R vendor/archify "$ARCH_HOME_DIR/vendor/archify"
echo "✓ archify: $ARCH_HOME_DIR/vendor/archify"

# 3. Ассеты/промпты/рубрики/скиллы и конфиг: встроены в бинарь — init офлайн.
"$BIN_DIR/arch-be" init

# 4. Прописать [archify].cli_path, если ещё не задан.
CFG="${XDG_CONFIG_HOME:-$HOME/.config}/arch-harness/config.toml"
CLI="$ARCH_HOME_DIR/vendor/archify/bin/archify.mjs"
if [ -f "$CFG" ] && ! grep -q '^cli_path = "[^"]\{1,\}"' "$CFG"; then
    if grep -q '^cli_path = ""' "$CFG"; then
        sed -i "s|^cli_path = \"\"|cli_path = \"$CLI\"|" "$CFG"
    else
        printf '\n[archify]\ncli_path = "%s"\n' "$CLI" >> "$CFG"
    fi
    echo "✓ archify cli_path → $CLI"
fi

echo
echo "Готово. Дальше — в корне проекта под контролем Spine:"
echo "  arch-be connect gigacode        # MCP + скиллы для GigaCode (или: claude/qwen/kimi/omp/codex)"
echo "  arch-be doctor --host gigacode  # проверка подключения"
INSTALL
chmod +x "$ROOT/install.sh"

# 3. Хэши всех файлов бандла (внутри архива; проверяются install.sh).
(
    cd "$ROOT"
    find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum | sed 's|\t \./|\t |; s|  \./|  |' > SHA256SUMS
)

# 4. Архив + хэш рядом (dist/SHA256SUMS — строка на бандл, как у релиза).
mkdir -p "$OUT_DIR"
TARBALL="$OUT_DIR/$BUNDLE_NAME.tar.gz"
tar -C "$STAGE" -czf "$TARBALL" "$BUNDLE_NAME"
(
    cd "$OUT_DIR"
    HASH="$(sha256sum "$BUNDLE_NAME.tar.gz")"
    if [ -f SHA256SUMS ]; then
        grep -v "  $BUNDLE_NAME.tar.gz\$" SHA256SUMS > SHA256SUMS.tmp || true
        mv SHA256SUMS.tmp SHA256SUMS
    fi
    echo "$HASH" >> SHA256SUMS
)

echo
echo "Готово:"
echo "  $TARBALL"
echo "  $(cat "$OUT_DIR/SHA256SUMS" | grep "$BUNDLE_NAME")"
echo "Проверка на чистом контуре: tar xzf $TARBALL -C /tmp && HOME=/tmp/<чистый> /tmp/$BUNDLE_NAME/install.sh"
