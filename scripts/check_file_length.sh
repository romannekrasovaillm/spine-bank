#!/usr/bin/env bash
# C-33 prod_file_length_limit: продовый Rust-код (src/**/*.rs) не длиннее
# 3000 строк на файл (критерий успеха волны B1 ревью 0.3.6). Нарушители
# перечисляются в stderr, выход ненулевой.
set -euo pipefail

LIMIT=3000
fail=0
while IFS= read -r -d '' f; do
    n=$(wc -l < "$f")
    if [ "$n" -gt "$LIMIT" ]; then
        echo "слишком длинный продовый модуль: $f — $n строк (> $LIMIT)" >&2
        fail=1
    fi
done < <(find src -type f -name '*.rs' -print0)

if [ "$fail" -ne 0 ]; then
    echo "подсказка: декомпозируй модуль на подмодули чистым перемещением с" >&2
    echo "реэкспортами в mod.rs (прецеденты: src/control/, src/gate/, src/cli/)" >&2
    exit 1
fi
