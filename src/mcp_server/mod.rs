//! MCP-серверный режим: `arch-be mcp serve` отдаёт архитектурный контроль
//! наружу кодовым агентам (Claude Code и др.) — verdict в момент написания
//! кода, а не на приёмке пакета (ADR-008, находка F-5 / задача P1-2).
//!
//! КОНТРАКТ (владелец: агент `mcp-serve`):
//! - транспорт stdio, NDJSON: одно сообщение JSON-RPC 2.0 — одна строка
//!   (как у клиента [`crate::mcp`], без `Content-Length`-фрейминга);
//!   stdout — только протокол, логи — stderr (tracing в `main`);
//! - методы: `initialize` (echo известной версии протокола, иначе наша),
//!   `tools/list`, `tools/call`, `prompts/list`, `prompts/get`, `ping`;
//!   `notifications/*` — игнор без ответа; неизвестный метод → `-32601`,
//!   битый JSON → `-32700`, отсутствует `method` → `-32600`, битые
//!   аргументы/инструмент/промпт → `-32602`; `resources/*` не поддержаны
//!   (`-32601`);
//! - промпты (capability `prompts`): десять плейбуков встроенного плагина
//!   spine-workflows как слэш-команды хоста ([`PLAYBOOK_PROMPTS`]) —
//!   хосту не нужно знать формулу «действуй по скиллу …» и то, куда он
//!   кладёт файлы скиллов: `prompts/get` возвращает user-сообщение с
//!   инструкцией и полным текстом SKILL.md. Текст — пользовательская
//!   копия из `plugins.dirs`, если есть (логика `skill_load`), иначе
//!   встроенный ассет: работает из коробки без `arch-be init`;
//! - режимы запуска ([`ServeMode`]): дефолт — строго read-only; флаг
//!   `--rw` (`arch-be mcp serve --rw`) дополнительно открывает белый список
//!   аддитивных записей ([`BRIDGE_READ_WRITE`]: `handoff_create`, `adr_new`,
//!   `agentsmd_generate`, `archify_deliver/show/compare`, `reverse_survey`,
//!   `skill_distill`, `evidence_pack`, `delta_propose`);
//! - инструменты — два слоя. РУЧНЫЕ (оттестированная поверхность ADR-008):
//!   контрольные `spine_lint`, `fitness_check`, `significance_score`,
//!   `significance_from_diff` (маршрут из git-диффа, S-1 anti-bypass),
//!   `trace_check`, `model_query`, `rubric_run` и чтение знаний (T4,
//!   ADR-015): `kb_search`, `skill_search`, `skill_load`, `mermaid_render`;
//!   плюс split-judge без LLM у сервера: `rubric_prompt` (промпты судьи +
//!   JSON-схема ответа) и `rubric_verify` (механическая сборка отчёта из
//!   сырых ответов хоста — медиана, `unstable`, `evidence_not_found`);
//!   плюс `rules_suggest` — кандидатные fitness-правила из пробелов кейса
//!   (EARS, таймауты контрактов, REQ→TASK, RTO/RPO→ADR, аудит операторских
//!   действий; модуль [`crate::rules_suggest`]).
//!   МОСТ: имена из белых списков [`BRIDGE_READ_ONLY`] (+ [`BRIDGE_READ_WRITE`]
//!   под `--rw`), не пересекающиеся с ручными, маршрутизируются в
//!   [`crate::tools::full_registry`] (`dispatch` — с политикой R-уровней;
//!   контекст БЕЗ LLM); спеки генерируются из `Tool::spec()`, annotations —
//!   из членства в списке + [`crate::policy::classify_tool`]. Транш 1
//!   инверсии в мосте: `nfr_check`, `model_validate`, `delta_guard`,
//!   `evidence_verify` (read-only верификаторы, JSON-вердикт
//!   passed/issues/summary в тексте вывода) и под `--rw` — `evidence_pack`,
//!   `delta_propose`. Транш 2: `landscape_report`, `adr_registry`,
//!   `rules_report`, `openspec_coverage`, `model_graph` (read-only отчёты:
//!   счётчики + markdown/mermaid в JSON; `passed=false` только у strict-гейтов
//!   `adr_registry`/`openspec_coverage`). Транш 3: `architect_review`,
//!   `change_impact` (составные инструменты — единое ревью репозитория и
//!   радиус изменения по графу модели; `src/review.rs`). В core-сборке (без
//!   фичи `harness`) домены
//!   `harness`/`distill`/`subagent`/`ralph`/`worktree`/`web` в реестре
//!   отсутствуют — мост их имена из белых списков молча пропускает (спеки
//!   строятся от реестра), `skill_distill` там недоступен;
//!   `handoff_create` — доступен и в core (генерация пакета — чисто
//!   файловая, модуль `crate::handoff`, волна 2 п.10);
//! - НИКОГДА не отдаются (даже под `--rw`) — [`BRIDGE_NEVER`]: write/exec/
//!   веб/субагенты (`bash`, `read_file`/`write_file`/`edit_file`, `glob`,
//!   `grep`, `propose_options`, `screenshot*`, `harness_run`, `subagent_*`,
//!   `ralph_run`, `worktree_new`, `web_*`) — это принадлежность хоста;
//!   `rubric_evaluate`/`rubric_generate` требуют LLM у сервера — вместо них
//!   split-judge. Решение политики Deny/RequireConfirm из `dispatch`
//!   возвращается как isError с текстом причины (подтверждение в
//!   неинтерактивном MCP невозможно → `RequireConfirm` трактуется как отказ);
//! - успешный вызов: `structuredContent` (машиночитаемый verdict) + тот же
//!   объект pretty-JSON в `content[0].text` (мостовые: text — сырой вывод
//!   инструмента, structuredContent — обёртка `{tool, output}`);
//!   контрольные verdict'ы несут `passed: bool` — `false` означает
//!   блокирующую находку, клиентский агент обязан отказать изменению,
//!   нарушающему `AD-*`;
//! - доменный сбой выполнения (файл не читается, сущность не найдена) —
//!   `result` с `isError: true`, не protocol error; сервер не падает ни на
//!   каком вводе, цикл живёт до EOF stdin;
//! - каждый вызов `tools/call` журналируется в проектный append-only журнал
//!   `<cwd сервера>/.arch-handoff/mcp-calls.jsonl` (модуль
//!   [`crate::mcp_journal`]: инструмент, вердикт, длительность, имена правил
//!   error-находок — БЕЗ содержимого аргументов; fail-soft, ротация по
//!   размеру) — источник outcome-данных для `arch-be digest`;
//! - `rubric_run` требует LLM-ключ из конфига: предпроверка доступности
//!   ключа (env задана / файл ключа существует; содержимое не печатается)
//!   → без ключа понятная JSON-RPC ошибка `-32603`; для моделей с
//!   `kind = "cli"` (внешний CLI-харнесс как LLM) ключ не нужен —
//!   предпроверка пропускается.

mod protocol;
#[cfg(test)]
mod testkit;
mod tools;
mod types;

pub use protocol::{serve, serve_with_mode};
pub use types::{
    BRIDGE_READ_ONLY, BRIDGE_READ_WRITE, MANUAL_TOOLS, McpServe, PATH_ARG_ALIASES, ServeMode,
};
