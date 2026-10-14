//! Реестр уроков (Lesson Ledger) — урок как сущность контура, а не строка в
//! контексте сессии (ADR-085, волна 1 задачи 0.5.0; п. L1).
//!
//! # Зачем
//!
//! Живые прогоны 0.4.10 показали: обучение в контексте уровня оркестратора
//! существует (0/17 повторов скрытой конвенции с контекстом против 8/8 без),
//! но держится на свободном тексте LLM, умирает с сессией и компактификацией,
//! не ловится `failure_memory` (B1, B2). Здесь урок получает долговечное
//! хранилище, область действия, доказательства с `run_id` и жизненный цикл.
//!
//! # Состав
//!
//! - [`ledger`] — сущность [`ledger::Lesson`], события журнала, свёртка
//!   проекции, пороги переходов, файловая блокировка дозаписи;
//! - [`scope`] — уровень области (`file|module|package|repo|user|org`) и
//!   детерминированное сопоставление глобов;
//! - [`render`] — байт-в-байт стабильный рендер `LESSONS.md` и атомарная
//!   запись.
//!
//! # Хранилище
//!
//! | Уровень | Путь |
//! |---|---|
//! | Репозиторий | `<repo>/.arch/lessons/ledger.jsonl` (+ `LESSONS.md`) |
//! | Пользователь | `$ARCH_HOME/lessons/ledger.jsonl` |
//! | Организация | экспорт/импорт (L10, ADR-091) |
//!
//! # Границы (AD-2)
//!
//! Модуль не обращается к LLM ни в одной ветке: статус урока — производная
//! от счётчиков доказательств, текст правила (`statement`) на статус не влияет
//! и может быть сформулирован моделью как `[inferred]`. Промоушен урока в
//! fitness-правило — отдельный слой (L7/ADR-089) с подписью человека на
//! маршрутах Standard/Critical.
//!
//! Явные вызовы (CLI и MCP) работают независимо от `[lessons] enabled`:
//! флаг гасит **автоматические** записи (гейт в петле исполнителя, L2; блок
//! инъекции, L3), а не действия человека.
//!
//! # Ошибки
//!
//! Предметные отказы (урок без доказательств, неизвестный id) возвращаются
//! как [`crate::HarnessError::Control`]: реестр — часть детерминированного
//! контура контроля, а не разбора конфигурации; ошибки разбора значений
//! (уровень, источник, исход, статус) — [`crate::HarnessError::Config`].

pub mod ledger;
pub mod render;
pub mod scope;

pub use ledger::{
    EventKind, Evidence, ID_PREFIX, LEDGER_DIR, LEDGER_FILE, LEDGER_VERSION, LESSONS_MD, Ledger,
    LedgerEvent, Lesson, LessonSource, LessonStatus, Mutation, Outcome, Provenance, RecordInput,
    Signature, Snapshot, Thresholds, now_iso, repo_ledger_path, today, user_dir, user_ledger_path,
    validate_imported,
};
pub use render::{render, render_lesson, write_atomic};
pub use scope::{Scope, ScopeLevel};

/// Версия arch-be — в провенанс каждой записи реестра (аудит: каким
/// бинарём урок записан).
#[must_use]
pub fn arch_be_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
