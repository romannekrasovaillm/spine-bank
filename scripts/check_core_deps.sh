#!/usr/bin/env bash
# C-34 core_boundary_no_tui_net: слим-профиль ядра
# (`cargo tree --no-default-features --features core -e normal`) не должен
# содержать опциональных harness-крейтов (TUI, сеть, буфер обмена,
# веб-парсинг). Это удерживаемая граница будущего выделения crates/spine-core
# (план 0.4.0, ADR-054): workspace-сплит без перепроводки фич возможен,
# только пока ядро не зависит от продуктовых крейтов. Нарушители
# перечисляются в stderr, выход ненулевой.
set -euo pipefail

# Опциональные крейты фичи `harness` (Cargo.toml: harness = ["dep:…"]).
# tokio НЕ в списке: это неопциональная зависимость ядра (процессы, MCP).
FORBIDDEN=(reqwest ratatui crossterm arboard scraper tokio-util)

tree=$(cargo tree --no-default-features --features core -e normal)

fail=0
for crate in "${FORBIDDEN[@]}"; do
    # Строка узла дерева: «├── имя vX.Y.Z» / «│   └── имя vX.Y.Z (*)».
    # Якорь « v<цифра>» после точного имени отсекает совпадения по подстроке
    # (tokio ≠ tokio-util, tokio-macros ≠ tokio).
    if grep -Eq "(^|[[:space:]])${crate} v[0-9]" <<<"$tree"; then
        echo "core-профиль тянет harness-крейт: $crate" >&2
        grep -nE "(^|[[:space:]])${crate} v[0-9]" <<<"$tree" >&2
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    echo "подсказка: верни гейтинг #[cfg(feature = \"harness\")] на модуль" >&2
    echo "или импорт нарушителя (прецеденты: src/tui/, src/llm/deepseek.rs," >&2
    echo "src/net.rs) либо вынеси использование за трейт-инжекцию (образцы:" >&2
    echo "&dyn LlmProvider в src/rubric/judge.rs, Tool в src/tool.rs)" >&2
    exit 1
fi
