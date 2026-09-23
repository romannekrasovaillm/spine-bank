//! Ручные инструменты MCP-сервера (нижний слой диспетчера — [`MANUAL_TOOLS`]
//! в `super::types`): по файлу на семейство — `fitness` (контур контроля и
//! маршрут значимости), `model` (запросы к типизированной модели), `rubric`
//! (LLM-судья и split-judge), `knowledge` (чтение знаний, T4), `insight`
//! (метрики контура: кандидаты правил, доверие, паспорт вердикта). Реализации —
//! `impl McpServe` в каждом файле; методы видны в пределах дерева `mcp_server`
//! (`pub(in crate::mcp_server)`), диспетчер — `super::protocol`.

mod fitness;
mod insight;
mod knowledge;
mod model;
mod rubric;
