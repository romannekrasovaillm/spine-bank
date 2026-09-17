---
name: spine-archify-viz
description: Визуализация архитектуры из харнесса через Archify (вендорен в Spine) — агент пишет JSON IR из кода/модели/заметок MD, итеративно чинит его по диагностикам archify_validate, получает интерактивный HTML через archify_show. Используй этот навык, когда просят «нарисуй архитектуру», «покажи диаграмму системы», «визуализируй модель/контракты/потоки», когда mermaid слишком беден (нужны зоны, guided views, легенды).
---

# Archify из харнесса: IR → validate → HTML

## Предусловие

В `arch-harness.toml` проекта (или конфиге Spine):

```toml
[archify]
cli_path = "<клон-spine>/vendor/archify/bin/archify.mjs"   # движок вендорен
```

Если `archify_validate` отвечает «archify не настроен» — скажи архитектору
эту строку (нужен Node.js ≥ 18).

## Протокол

1. Собери факты: `model_query` (сущности модели), контракты, README — НЕ
   выдумывай топологию.
2. Напиши IR `diagrams/<имя>.architecture.json` (типы: `architecture`,
   `workflow`, `sequence`, `dataflow`, `lifecycle`). Минимум: meta
   (title — по-русски ок), nodes (id/kind/label), edges (from/to/label),
   zones для границ доверия.
3. `archify_validate {"type": "architecture", "path": "...", "cwd": "."}` →
   9 артефактных проверок + composition. При провале читай `diagnostics` —
   там `supportedFixes`: применяй ТОЧЕЧНО и перепроверяй. Не переписывай IR
   с нуля из-за одной диагностики.
4. Чисто (9/9) → `archify_show` (доставка HTML + receipt SHA-256;
   в `--rw`-режиме сервера) или `archify_deliver` для приёмки.
5. Доложи: путь к HTML, число проверок, что осознанно не делал
   (visual-check, перцептивное ревью — отдельный прогон).

## Когда НЕ Archify

Быстрая блок-схема в ответе — `mermaid_render {"path": "…mmd"}` (или `code`)
достаточно; Archify — для презентационных/ревью-артefactов.
