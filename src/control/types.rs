//! Типы данных контура контроля: модель правила ([`FitnessRule`],
//! [`RuleKind`]), контракт отчёта ([`FitnessReport`], [`LintIssue`],
//! [`RuleDuration`], [`RulesFingerprint`], [`RunnerSkippedRule`],
//! [`UntrustedSkippedRule`], [`SkippedUnknownRule`]), карточка правила
//! ([`RuleCard`]) и общая нормализация severity ([`normalize_severity`]).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::baseline;
use crate::error::{HarnessError, Result};

/// Маршрут изменения по значимости.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Route {
    /// Низкий риск: дельта-спека + авто-валидация.
    Fast,
    /// Средний: контракт Spec→Plan→Tasks + Architecture Fit автоматически.
    Standard,
    /// Архитектурно/регуляторно значимое: Solutioning + human decision (A3).
    Critical,
}

impl std::str::FromStr for Route {
    type Err = String;

    /// Парсит маршрут из строки (fast/standard/critical, без учёта регистра).
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "standard" => Ok(Self::Standard),
            "critical" => Ok(Self::Critical),
            other => Err(format!(
                "неизвестный маршрут '{other}' (допустимы: fast, standard, critical)"
            )),
        }
    }
}

impl std::fmt::Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Fast => "Fast",
            Self::Standard => "Standard",
            Self::Critical => "Critical",
        };
        f.write_str(s)
    }
}

/// Результат оценки значимости.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Significance {
    /// Число сработавших триггеров.
    pub score: usize,
    /// Сработавшие триггеры.
    pub fired: Vec<String>,
    /// Маршрут.
    pub route: Route,
}

/// Находка линтера/сенсора.
///
/// Карточные поля (`ad`…`skill`) — архитектурный контекст находки: проставляются
/// движком `control check` из карточки породившего правила (`CONSTRAINTS.yaml`),
/// у находок линтера spine/дельты/наследования их нет. Аддитивный контракт
/// (SDK v1): отсутствующие поля не сериализуются, старые клиенты не ломаются.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LintIssue {
    /// Файл.
    pub file: PathBuf,
    /// Строка (0 — файл целиком).
    pub line: usize,
    /// Код правила (`dup_ad_id`, `empty_field`, `stub_marker`, `unpinned_version`,
    /// `broken_ad_ref` либо имя fitness-правила из `CONSTRAINTS.yaml`).
    pub rule: String,
    /// Сообщение.
    pub message: String,
    /// Критичность: error|warn.
    pub severity: String,
    /// Задетый инвариант spine (`AD-<n>`, из карточки правила).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ad: Option<String>,
    /// Связанное архитектурное решение (`ADR-<n>`, из карточки правила).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adr: Option<String>,
    /// Какой отказ предотвращает правило (из карточки).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// Владелец правила (из карточки).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Подсказка исправления (из карточки правила).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_hint: Option<String>,
    /// Скилл библиотеки плагинов, который загрузить для исправления
    /// (из карточки правила).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
}

/// Длительность выполнения одного fitness-правила (per-rule timing).
///
/// Аддитивное поле отчёта (SDK-контракт v1: новые поля не ломают клиентов).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDuration {
    /// Имя правила.
    pub rule: String,
    /// Длительность, миллисекунды.
    pub ms: u64,
}

/// Отпечаток состава реестра правил (П5 ДКА): SHA-256 отсортированного
/// набора `id|name|severity` плюс счётчики. Делает строку «14 правил» при
/// вчерашних 15 самостоятельным сигналом, а не молчаливым зелёным.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesFingerprint {
    /// SHA-256 состава правил (полный hex).
    pub hash: String,
    /// Число правил в реестре.
    pub rules: usize,
    /// Число error-правил.
    pub errors: usize,
}

/// Правило, не прогонявшееся из-за отсутствия внешнего прогонщика (A2):
/// pytest/mvn/JDK нет в PATH — правило пропущено с причиной и подсказкой по
/// установке, а не провалено. Гейт переводит пропуск error-правила в SKIP
/// составляющей `fitness` (вердикт неполон → INCOMPLETE), warn-правила
/// остаются заметкой в текстовом выводе.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerSkippedRule {
    /// Имя правила.
    pub rule: String,
    /// Нормализованный severity правила (`error`/`warn`).
    pub severity: String,
    /// Отсутствующие прогонщики (метки `pytest`, `mvn`, `JDK (java/javac)`).
    pub runners: Vec<String>,
    /// Причина и подсказка по установке (с префиксом
    /// [`crate::rule_templates::RUNNER_ABSENT_PREFIX`]).
    pub reason: String,
}

/// Правило, не прогонявшееся по решению модели доверия (A3, ADR-053):
/// активен no-exec (флаг/переменная/дефолт MCP) либо allow-файл
/// `trusted.json` не совпадает с реестром (реестр менялся после доверия).
/// Пропуск с причиной `command_untrusted`, а не провал: команда НЕ
/// запускалась вообще. Гейт переводит такой пропуск ЛЮБОГО severity в SKIP
/// составляющей `fitness` (строже A2: пропуск по решению о доверии —
/// событие политики, а не разрыв окружения).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntrustedSkippedRule {
    /// Имя правила.
    pub rule: String,
    /// Нормализованный severity правила (`error`/`warn`).
    pub severity: String,
    /// Причина и подсказка, как разрешить исполнение (с префиксом
    /// [`crate::cmd_trust::COMMAND_UNTRUSTED`]).
    pub reason: String,
}

/// Правило, не прогонявшееся из-за недоступного ресурса среды (`requires:`,
/// ADR-068): правило объявило `cuda`/`stand`, а среда их не предоставляет —
/// SKIP с причиной, а не PASS (зелёный на неполном окружении — ложь) и не
/// FAIL (наказание за отсутствие железа). Гейт переводит такой пропуск в SKIP
/// составляющей `fitness` при любом severity и печатает
/// [`crate::control::requires::STAND_RUN_LINE`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiresSkippedRule {
    /// Имя правила.
    pub rule: String,
    /// Нормализованный severity правила (`error`/`warn`).
    pub severity: String,
    /// Недоступные ресурсы из `requires` (порядок объявления).
    pub resources: Vec<String>,
    /// Причина и обязательная строка прогона на стенде.
    pub reason: String,
}

/// Счётчик правил по источнику (вывод `check` и `report`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCount {
    /// Метка источника (`<ref>@<version>` либо `own` — собственные).
    pub source: String,
    /// Число правил.
    pub rules: usize,
}

/// Отчёт fitness-контроля.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitnessReport {
    /// Репозиторий.
    #[serde(alias = "path")]
    pub repo: PathBuf,
    /// Все правила пройдены.
    pub passed: bool,
    /// Находки по правилам.
    pub issues: Vec<LintIssue>,
    /// Сводка (для отчёта).
    pub summary: String,
    /// Длительность каждого правила (порядок — как в `CONSTRAINTS.yaml`).
    #[serde(default)]
    pub durations: Vec<RuleDuration>,
    /// Источники правил при наследовании `extends` (метка → число правил;
    /// пусто, если наследования нет). Аддитивное поле SDK-контракта v1.
    #[serde(default)]
    pub inherited: Vec<SourceCount>,
    /// Overrides из `CONSTRAINTS.yaml` со статусами (active/expired/invalid;
    /// `docs/corp-spine.md`). Аддитивное поле SDK-контракта v1.
    #[serde(default)]
    pub overrides: Vec<OverrideInfo>,
    /// Итог ratchet-сравнения с baseline (`--baseline`; модуль [`baseline`]).
    /// `None` — прогон без baseline. Аддитивное поле SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<baseline::BaselineReport>,
    /// Правила, пропущенные в режиме `--changed-since` (глобальные — по
    /// дизайну, файловые — при пустом срезе их файлов), с причинами.
    /// Аддитивное поле SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<baseline::SkippedRule>,
    /// Git-реф режима `--changed-since` (`None` — полный прогон).
    /// Аддитивное поле SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_since: Option<String>,
    /// Число изменённых файлов в срезе `--changed-since`.
    /// Аддитивное поле SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<usize>,
    /// Правила, пропущенные при разборе реестра из-за неизвестного типа
    /// (словарь другой редакции, E8): отражены warn-находками
    /// `unknown_rule_type`, вердикт не ломают. Аддитивное поле SDK-контракта
    /// v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_unknown: Vec<SkippedUnknownRule>,
    /// Правила, не прогонявшиеся из-за отсутствия внешнего прогонщика
    /// (pytest/mvn/JDK, A2): пропуск с причиной, а не находка — в `issues`
    /// не попадают и `passed` не меняют. Аддитивное поле SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runner_skipped: Vec<RunnerSkippedRule>,
    /// Правила, не прогонявшиеся по решению модели доверия (A3, ADR-053):
    /// no-exec или allow-файл не совпадает с реестром — пропуск с причиной
    /// `command_untrusted`, а не находка. Аддитивное поле SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untrusted_skipped: Vec<UntrustedSkippedRule>,
    /// Правила, не прогонявшиеся из-за недоступного ресурса среды
    /// (`requires:`, ADR-068): SKIP с причиной, а не находка. Аддитивное поле
    /// SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_skipped: Vec<RequiresSkippedRule>,
    /// Отпечаток состава реестра правил (П5). Аддитивное поле SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<RulesFingerprint>,
    /// Правила со статусом N/A (волна S, ADR-070): объявленные условия
    /// `applies_to` не выполнены — правило не прогонялось, гейт не ломает.
    /// Аддитивное поле SDK-контракта v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_applicable: Vec<NotApplicableRule>,
}

/// Тип fitness-правила из `CONSTRAINTS.yaml`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuleKind {
    /// Regex должен найтись хотя бы в одном файле по glob.
    MustContain,
    /// Regex не должен встречаться ни в одном файле по glob (issue на каждое вхождение).
    MustNotContain,
    /// Regex обязан найтись в КАЖДОМ файле по glob (issue на каждый файл без
    /// совпадения). Пустой набор файлов — находка (glob, скорее всего, ошибочен).
    EachFileMustContain,
    /// Файл существует относительно корня репозитория.
    FileExists,
    /// В каждом каталоге по glob существует файл `path` (например, у каждого
    /// сервиса `services/*` есть `openapi.yaml`). Пустой набор каталогов —
    /// находка.
    DirMustHaveFile,
    /// Файл существует и свеж: его mtime не старше `max_age_days` дней
    /// (свежесть evidence: дата последних учений, ежегодный pentest).
    MaxAge,
    /// Команда (`bash -c`, в корне репозитория) завершается кодом 0 до таймаута.
    CommandSucceeds,
    /// Структурная проверка направления зависимостей (ADR-029): импорты
    /// каждого файла набора сопоставляются с `forbid`/`allow`-списками
    /// модулей (префикс модульного пути по сегментам). Пустой набор файлов —
    /// находка.
    DependencyDirection,
    /// Границы контекстов (ADR-030): импорты файлов не пересекают
    /// `code_roots` чужих CMP-сущностей модели (`model_dir`), если целевой
    /// CMP не объявлен в `depends_on` исходного.
    ContextBoundary,
    /// `ArchUnit`-гейт (ADR-039): java-правила этого же `CONSTRAINTS.yaml`
    /// (`dependency_direction`/`context_boundary` с glob `**/*.java`)
    /// исполняются настоящим `ArchUnit` на скомпилированных классах
    /// (standalone-раннер, общий код с `arch-be archunit check`).
    #[serde(rename = "archunit")]
    ArchUnit,
    /// Запрещённые пакеты в манифестах зависимостей (детектор тех-радара,
    /// `docs/corp-spine.md`): построчный разбор `Cargo.toml` (секции
    /// *dependencies), `pom.xml` (`<artifactId>`), `requirements.txt`;
    /// пакет из `deny` — находка со ссылкой `reference` на решение
    /// техкомитета.
    DenyDependency,
    /// Условное правило (волна S, ADR-070): `if: {glob, pattern}` —
    /// триггер; если в пределах модуля (каталог с манифестом сборки) нашёлся
    /// хотя бы один файл по `if.glob` с `if.pattern`, то в том же модуле
    /// обязан найтись файл по `then.glob` с `then.pattern`. Выражает S5
    /// «исходящие вызовы → circuit breaker».
    Conditional,
    /// Разрешённые исходящие хосты (волна S, ADR-070): URL-литералы
    /// (`scheme://host`) в файлах по `glob` сверяются с реестром интеграций
    /// (`allow` — список хостов). Хост вне реестра — находка (S10).
    OutboundAllowlist,
    /// Владение хранилищем (волна S, S9, ADR-078): `store:`-узлы графа
    /// as-built (ADR-069) читают/пишут только компоненты-владельцы. Режим
    /// whitelist (`owners: [CMP-…]`) — обращение чужого компонента к
    /// хранилищу владельца есть находка; режим без `owners` —
    /// «database-per-service»: у хранилища не более одного компонента-
    /// коннектора (второе ребро `connect` — находка).
    DataOwnership,
}

impl RuleKind {
    /// Строковое имя типа (как в YAML) — для таблиц отчётов. `pub(crate)`:
    /// по нему же паспорт вердикта (W1) отличает правила, проверяющие
    /// поведение, от правил на упоминание.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MustContain => "must_contain",
            Self::MustNotContain => "must_not_contain",
            Self::EachFileMustContain => "each_file_must_contain",
            Self::FileExists => "file_exists",
            Self::DirMustHaveFile => "dir_must_have_file",
            Self::MaxAge => "max_age",
            Self::CommandSucceeds => "command_succeeds",
            Self::DependencyDirection => "dependency_direction",
            Self::ContextBoundary => "context_boundary",
            Self::ArchUnit => "archunit",
            Self::DenyDependency => "deny_dependency",
            Self::Conditional => "conditional",
            Self::OutboundAllowlist => "outbound_allowlist",
            Self::DataOwnership => "data_ownership",
        }
    }

    /// Файловое ли правило (content-правило по glob-набору файлов): только
    /// они исполняются в режиме `--changed-since` — на срезе изменённых
    /// файлов; глобальные и структурные правила на срезе лгут или дороги и
    /// пропускаются с пометкой в отчёте (модуль [`baseline`]).
    pub(super) fn is_file_scoped(self) -> bool {
        matches!(
            self,
            Self::MustContain | Self::MustNotContain | Self::EachFileMustContain
        )
    }
}

/// Применимость правила к репозиторию (волна S, ADR-070): правило объявляет,
/// к каким стекам и структурам оно относится.
///
/// Неприменимое правило получает статус **N/A** (не FAIL и не PASS): корп-
/// стандарт, написанный под Java-Maven, не краснит Python-репозиторий ложной
/// находкой «правилу нечего проверять». Отсутствие условий (или отсутствие
/// поля `applies_to`) — правило применимо всегда: поведение существующих
/// правил не меняется.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct AppliesTo {
    /// Стеки, к которым применимо правило (`java-maven`, `java-gradle`,
    /// `node`, `python`, `rust`, `dotnet`, `go`). Пусто — любой стек.
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub(crate) stack: Vec<String>,
    /// Glob'ы: правило применимо, только если существует хотя бы один файл
    /// под одним из них (например, `**/pom.xml`). Пусто — без условия.
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub(crate) when_exists: Vec<String>,
    /// Применимо, только если собственный glob-набор правила (после
    /// исключений) непуст; пустой набор — N/A, а не находка «правилу нечего
    /// проверять».
    #[serde(default)]
    pub(crate) when_glob_nonempty: bool,
}

/// Правило, неприменимое к репозиторию (волна S, ADR-070): объявленные
/// условия [`AppliesTo`] не выполнены — статус N/A (не PASS и не FAIL).
/// НЕ находка, гейт не ломает; перечисляется в отчёте покрытия стандарта.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotApplicableRule {
    /// Имя правила.
    pub rule: String,
    /// Нормализованный severity правила (`error`/`warn`).
    pub severity: String,
    /// Причина неприменимости (какое условие не выполнено).
    pub reason: String,
}

/// Одна запись `deny`-списка тех-радара (`deny_dependency`, волна S, ADR-070).
///
/// Простая форма — `h2` (имя пакета); развёрнутая — `{name: log4j-core,
/// below: "2.17.1", scopes: [runtime, compile]}` (диапазон версий и фильтр
/// scope). Формат — dual: строка задаёт имя; карта — имя плюс необязательные
/// диапазон и scope. Отсутствие развёрнутых полей сохраняет семантику
/// существующих `deny: [pkg, ...]` (сравнение только по имени).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum DenySpec {
    /// Простое имя пакета (обратная совместимость).
    Name(String),
    /// Развёрнутая запись: имя + диапазон версии + scope-фильтр.
    Detailed {
        /// Имя пакета (артефакт; для `groupId:artifactId` — проверяется и
        /// полная координата, и последний сегмент).
        name: String,
        /// Запрещать версии строго НИЖЕ порога (`< below`); пусто — любая
        /// версия. Непарсимая версия НЕ краснит (fail-open).
        #[serde(default)]
        below: Option<String>,
        /// Разрешённый набор scope для краснения (`compile`/`runtime`/`test`/
        /// `peer`/`optional`); пусто — любой scope.
        #[serde(default, deserialize_with = "de_string_or_list")]
        scopes: Vec<String>,
    },
}

impl DenySpec {
    /// Имя пакета записи.
    pub(crate) fn name(&self) -> &str {
        match self {
            DenySpec::Name(name) | DenySpec::Detailed { name, .. } => name,
        }
    }

    /// Порог диапазона версий (`below`) — `None`, если не задан.
    pub(crate) fn below(&self) -> Option<&str> {
        match self {
            DenySpec::Name(_) => None,
            DenySpec::Detailed { below, .. } => below.as_deref(),
        }
    }

    /// Набор scope для краснения — пусто, если не задан.
    pub(crate) fn scopes(&self) -> &[String] {
        match self {
            DenySpec::Name(_) => &[],
            DenySpec::Detailed { scopes, .. } => scopes,
        }
    }

    /// Канонический текст записи для анти-ослабления (A1): порядок scope не
    /// влияет на хэш тела правила.
    fn canonical(&self) -> String {
        let mut scopes: Vec<&str> = self.scopes().iter().map(String::as_str).collect();
        scopes.sort_unstable();
        format!(
            "{}|below={}|scopes={}",
            self.name(),
            self.below().unwrap_or_default(),
            scopes.join(",")
        )
    }
}

/// Условие/обязательство правила `conditional` (волна S, ADR-070): набор
/// файлов (`glob`) и regex (`pattern`). Используется и для триггера `if`
/// (модуль активируется, если в нём есть файл по glob с pattern), и для
/// обязательства `then` (в активированном модуле обязан найтись такой файл).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ConditionalClause {
    /// Glob'ы модуля-локального поиска (строка или список); пусто — `**/*`.
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub(super) glob: Vec<String>,
    /// Regex, наличие которого в файле по `glob` активирует/удовлетворяет
    /// условие.
    pub(super) pattern: String,
}

/// Одно правило из `CONSTRAINTS.yaml`.
///
/// Публичная структура (нужна `ArchUnit`-мосту на CLI-краю, ADR-039); поля
/// крейт-видимые — внешние потребители работают через [`check`] и JSON-отчёт.
#[derive(Debug, Deserialize)]
pub struct FitnessRule {
    /// Имя правила (становится кодом находки).
    pub(crate) name: String,
    /// Идентификатор правила (например, `C-12`; dogfood-набор и библиотека
    /// правил). Используется `ArchUnit`-мостом (ADR-039) как id правила в
    /// сообщениях; движок находок по-прежнему оперирует `name`.
    #[serde(default)]
    pub(crate) id: Option<String>,
    /// Тип проверки. Может опускаться у записей `unverifiable: true`
    /// (ручной контроль, `docs/corp-spine.md`) — движок их не исполняет.
    #[serde(rename = "type", default = "default_rule_kind")]
    pub(crate) kind: RuleKind,
    /// Glob'ы набора файлов (для content-правил, `dependency_direction` и
    /// `context_boundary`) и каталогов (для `dir_must_have_file`); строка
    /// или список строк, дефолт `**/*`. Наборы по нескольким glob'ам
    /// объединяются (ADR-029: слоевые правила покрывают `src/x.rs` +
    /// `src/x/**` одним правилом).
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub(crate) glob: Vec<String>,
    /// Regex (для `must_contain/must_not_contain/each_file_must_contain`).
    pub(super) pattern: Option<String>,
    /// Путь относительно репозитория (для `file_exists` и `max_age`) или
    /// относительно каждого каталога набора (для `dir_must_have_file`).
    pub(super) path: Option<String>,
    /// Glob'ы исключений из набора (строка или список строк; для
    /// content-правил и `dir_must_have_file`). Файлы/каталоги, попавшие под
    /// исключение, вычитаются из набора ДО проверки — легитимные точечные
    /// отступления (например, сборщик витрины, которому JOIN разрешён).
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub(super) exclude_glob: Vec<String>,
    /// Максимальный возраст файла в днях (для `max_age`; mtime не старше
    /// `now − max_age_days`).
    pub(super) max_age_days: Option<u64>,
    /// Команда (для `command_succeeds`).
    pub(super) command: Option<String>,
    /// Запрещённые модули (для `dependency_direction`, ADR-029): префиксы
    /// модульного пути в координатах импортов (`agent`, `com/bank/legacy`).
    /// Ровно одно из `forbid`/`allow` обязательно.
    pub(crate) forbid: Option<Vec<String>>,
    /// Разрешённые модули (для `dependency_direction`): импорт обязан
    /// префиксно совпадать с одним из них; пустой список — запрет любых
    /// внутрикрейтовых зависимостей (листовые модули).
    pub(crate) allow: Option<Vec<String>>,
    /// Каталог модели архитектуры относительно корня репозитория (для
    /// `context_boundary`, ADR-030; дефолт `model`).
    pub(crate) model_dir: Option<String>,
    /// Каталог скомпилированных классов JVM-проекта относительно корня
    /// репозитория (для `archunit`, ADR-039; без него — авто-детект
    /// `target/classes`, `build/classes/java/main`, `out/production`,
    /// `classes`).
    #[serde(default)]
    pub(crate) classes_dir: Option<String>,
    /// Каталог с jar'ами `ArchUnit` относительно корня репозитория (для
    /// `archunit`; без него — `--jar-dir` / `ARCHUNIT_HOME` /
    /// `~/.arch-harness/archunit/lib`).
    #[serde(default)]
    pub(crate) jar_dir: Option<String>,
    /// Карточка правила (метаданные из шаблона дистилляции источников,
    /// необязательны): признак применимости.
    /// Поля схемы — читаются внешними потребителями YAML; движок находок
    /// использует `ad`/`adr`/`rationale`/`owner`/`fix_hint`/`skill`
    /// (переносит в находки) и expiry (expiry-находка).
    #[serde(default)]
    #[allow(dead_code)]
    trigger: Option<String>,
    /// Карточка правила: какой отказ предотвращается (одно предложение).
    /// Переносится движком в находки (`LintIssue::rationale`).
    #[serde(default)]
    rationale: Option<String>,
    /// Карточка правила: задетый инвариант spine (`AD-<n>`). Переносится
    /// движком в находки (`LintIssue::ad`).
    #[serde(default)]
    pub(super) ad: Option<String>,
    /// Карточка правила: связанное архитектурное решение (`ADR-<n>`).
    /// Переносится движком в находки (`LintIssue::adr`).
    #[serde(default)]
    adr: Option<String>,
    /// Карточка правила: подсказка исправления (что сделать вместо
    /// нарушения). Переносится движком в находки (`LintIssue::fix_hint`).
    #[serde(default)]
    fix_hint: Option<String>,
    /// Карточка правила: скилл библиотеки плагинов для исправления
    /// (`skill_load <имя>`). Переносится движком в находки (`LintIssue::skill`).
    #[serde(default)]
    skill: Option<String>,
    /// Карточка правила: артефакт, остающийся после проверки.
    #[serde(default)]
    #[allow(dead_code)]
    evidence: Option<String>,
    /// Карточка правила: стоимость отмены (обратимо / дорого / необратимо).
    #[serde(default)]
    #[allow(dead_code)]
    reversibility: Option<String>,
    /// Карточка правила: владелец. Переносится движком в находки
    /// (`LintIssue::owner`) и читается обходом `change_impact`
    /// (`src/review.rs`) — «с кем согласовывать».
    #[serde(default)]
    pub(crate) owner: Option<String>,
    /// Карточка правила: дата пересмотра (YYYY-MM-DD). Просроченное правило —
    /// находка уровня warn (антипаттерн «правило без срока жизни» — теперь
    /// механически видно).
    #[serde(default)]
    pub(crate) expiry: Option<String>,
    /// Карточка правила: оценка стоимости сопровождения в человеко-часах
    /// (метаданные; движок не enforce'ит — суммируется в `rules_report`).
    #[serde(default)]
    pub(super) effort_hours: Option<f64>,
    /// Связь «правило ← требование» (адаптер `OpenSpec`, `src/openspec.rs`):
    /// идентификаторы требований вида `openspec:<capability>#<hash8>`,
    /// которые покрывает это правило. Движок находок поле не использует;
    /// читает отчёт покрытия `arch-be openspec coverage`.
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) covers: Vec<String>,
    /// Ресурсы среды, необходимые правилу (ADR-068): `cuda` (GPU) и/или
    /// `stand` (стенд GB10/Spark/Grace). Если ресурс недоступен, правило даёт
    /// SKIP с причиной, а не PASS/FAIL. Пусто (дефолт) — правило исполняется
    /// на любой машине (поведение существующих правил не меняется).
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub(crate) requires: Vec<String>,
    /// Glob'ы манифестов зависимостей (для `deny_dependency`; строка или
    /// список). Пустой — auto: `**/Cargo.toml`, `**/pom.xml`,
    /// `**/requirements.txt`.
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub(super) manifests: Vec<String>,
    /// Запрещённые пакеты (для `deny_dependency`, волна S, ADR-070): строка,
    /// список имён или список развёрнутых записей [`DenySpec`] с диапазоном
    /// версий и scope-фильтром.
    #[serde(default, deserialize_with = "de_deny_list")]
    pub(super) deny: Vec<DenySpec>,
    /// Ссылка на основание запрета — решение техкомитета/ADR (для
    /// `deny_dependency`; попадает в текст находки).
    #[serde(default)]
    pub(super) reference: Option<String>,
    /// Метка источника правила при наследовании (`extends`,
    /// `docs/corp-spine.md`): `<ref>@<version>` родительского файла;
    /// `None` — собственное правило этого файла. Из YAML не читается,
    /// проставляется резолвером [`load_constraints_resolved`].
    #[serde(skip)]
    pub(crate) source: Option<String>,
    /// Признак ручного контроля (`docs/corp-spine.md`): требование/стандарт
    /// не механизуется — движок правило НЕ исполняет, но оно видно в
    /// отчётах (`control report`, поле `unverifiable_rules`) как осознанный
    /// долг с owner. Поле `type` у таких записей можно опускать.
    #[serde(default)]
    pub(crate) unverifiable: bool,
    /// Критичность находок правила: error|warn (дефолт error).
    #[serde(default = "default_severity")]
    pub(crate) severity: String,
    /// Таймаут исполнения, секунды. Дефолт по типу правила: 60 для
    /// `command_succeeds`, 300 для `archunit` (ADR-039).
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
    /// Применимость к репозиторию (волна S, ADR-070): объявленные условия
    /// (`stack`/`when_exists`/`when_glob_nonempty`). Неприменимое правило даёт
    /// N/A, а не находку. Отсутствие поля — правило применимо всегда.
    #[serde(default)]
    pub(crate) applies_to: Option<AppliesTo>,
    /// Многострочный режим content-правила (волна S, ADR-070): `must_not_contain`
    /// ищет совпадение по содержимому файла целиком (regex может захватывать
    /// переносы строк), а не построчно. Выражает S12 «нет field injection»
    /// (аннотация над полем — многострочный шаблон). Дефолт `false` —
    /// построчная семантика существующих правил не меняется.
    #[serde(default)]
    pub(super) multiline: bool,
    /// Триггер условного правила (для `conditional`, волна S, ADR-070).
    #[serde(rename = "if", default)]
    pub(super) if_cond: Option<ConditionalClause>,
    /// Обязательство условного правила (для `conditional`, волна S, ADR-070).
    #[serde(rename = "then", default)]
    pub(super) then_cond: Option<ConditionalClause>,
    /// Компоненты-владельцы хранилищ (для `data_ownership`, волна S, S9,
    /// ADR-078): id (`CMP-…`) или имена компонентов, которым разрешено
    /// обращение к `store:`-узлам графа as-built. Пусто — режим
    /// «database-per-service» (у хранилища не более одного коннектора).
    #[serde(default, deserialize_with = "de_string_or_list")]
    pub(super) owners: Vec<String>,
}

impl FitnessRule {
    /// Переносит архитектурный контекст карточки правила в находку
    /// (`ad`/`adr`/`rationale`/`owner`/`fix_hint`/`skill`): агент видит задетый
    /// инвариант и подсказку исправления, а не только имя правила. Поля,
    /// пустые в карточке, остаются `None` (в JSON не сериализуются).
    pub(super) fn apply_card(&self, issue: &mut LintIssue) {
        issue.ad.clone_from(&self.ad);
        issue.adr.clone_from(&self.adr);
        issue.rationale.clone_from(&self.rationale);
        issue.owner.clone_from(&self.owner);
        issue.fix_hint.clone_from(&self.fix_hint);
        issue.skill.clone_from(&self.skill);
    }

    /// Карточка правила как текст — что правило **проверяет**: набор файлов,
    /// шаблон или команду, серьёзность и заявленный инвариант.
    ///
    /// Зачем отдельным текстом: в досье судьи (ADR-051, рубрика
    /// `model_link_semantics`) ссылка `AD-1 → C-09` без карточки бессмысленна —
    /// идентификатор правила не говорит, относится ли проверка к формулировке
    /// инварианта или к чему-то совсем другому.
    #[must_use]
    pub fn card(&self) -> String {
        let mut out = String::new();
        let id = self.id.as_deref().unwrap_or(&self.name);
        let _ = writeln!(out, "{id}: {}", self.name); // игнорируется: записи в String не падают
        let _ = writeln!(out, "Тип: {}", self.kind.as_str()); // игнорируется: записи в String не падают
        let _ = writeln!(out, "Серьёзность: {}", self.severity); // игнорируется: записи в String не падают
        if !self.glob.is_empty() {
            let _ = writeln!(out, "Набор файлов: {}", self.glob.join(", ")); // игнорируется: записи в String не падают
        }
        if let Some(path) = &self.path {
            let _ = writeln!(out, "Путь: {path}"); // игнорируется: записи в String не падают
        }
        if let Some(pattern) = &self.pattern {
            let _ = writeln!(out, "Шаблон (regex): {pattern}"); // игнорируется: записи в String не падают
        }
        if let Some(command) = &self.command {
            let _ = writeln!(out, "Команда: {command}"); // игнорируется: записи в String не падают
        }
        if let Some(ad) = &self.ad {
            let _ = writeln!(out, "Заявленный инвариант: {ad}"); // игнорируется: записи в String не падают
        }
        if let Some(rationale) = &self.rationale {
            let _ = writeln!(out, "Зачем: {rationale}"); // игнорируется: записи в String не падают
        }
        out
    }

    /// Поля ТЕЛА правила для анти-ослабления (A1): всё, что определяет
    /// проверку, кроме карточки (`rationale`/`fix_hint`/`skill`/`ad`),
    /// `severity` и `exclude_glob`.
    ///
    /// Последние два сравниваются отдельными нормализованными правилами
    /// (`SeverityLowered`/`ExcludeWidened`): их присутствие в теле дало бы
    /// двойную находку, а сырой `severity` — ложное «ослабление» на
    /// эквивалентах (`critical` ≡ `error`).
    ///
    /// Ключи отсортированы (`BTreeMap`) — каноническая сериализация: хэш
    /// набора не зависит от порядка полей в YAML. Списки сортируются и
    /// соединяются разделителем `\u{1f}`; отсутствие значения — пустая строка
    /// (`None` и пустая строка неразличимы намеренно: «добавили пустое поле» —
    /// не изменение проверки).
    #[must_use]
    pub(crate) fn body_fields(&self) -> BTreeMap<&'static str, String> {
        fn list(values: &[String]) -> String {
            let mut refs: Vec<&str> = values.iter().map(String::as_str).collect();
            refs.sort_unstable();
            refs.join("\u{1f}")
        }
        fn opt_list(values: &Option<Vec<String>>) -> String {
            values.as_deref().map_or(String::new(), list)
        }
        fn deny_specs(values: &[DenySpec]) -> String {
            let mut refs: Vec<String> = values.iter().map(DenySpec::canonical).collect();
            refs.sort_unstable();
            refs.join("\u{1f}")
        }
        fn opt(value: &Option<String>) -> String {
            value.clone().unwrap_or_default()
        }
        fn num<T: ToString>(value: Option<T>) -> String {
            value.map_or(String::new(), |v| v.to_string())
        }
        fn flag(value: bool) -> String {
            if value { "true".into() } else { String::new() }
        }
        fn applies(a: &Option<AppliesTo>) -> String {
            match a {
                None => String::new(),
                Some(a) => format!(
                    "stack={};when_exists={};when_glob_nonempty={}",
                    list(&a.stack),
                    list(&a.when_exists),
                    flag(a.when_glob_nonempty)
                ),
            }
        }
        fn clause(c: &Option<ConditionalClause>) -> String {
            match c {
                None => String::new(),
                Some(c) => format!("glob={};pattern={}", list(&c.glob), c.pattern),
            }
        }
        let mut out = BTreeMap::new();
        out.insert("id", opt(&self.id));
        out.insert("kind", self.kind.as_str().to_string());
        out.insert("glob", list(&self.glob));
        out.insert("pattern", opt(&self.pattern));
        out.insert("path", opt(&self.path));
        out.insert("max_age_days", num(self.max_age_days));
        out.insert("command", opt(&self.command));
        out.insert("forbid", opt_list(&self.forbid));
        out.insert("allow", opt_list(&self.allow));
        out.insert("owners", list(&self.owners));
        out.insert("model_dir", opt(&self.model_dir));
        out.insert("classes_dir", opt(&self.classes_dir));
        out.insert("jar_dir", opt(&self.jar_dir));
        out.insert("deny", deny_specs(&self.deny));
        out.insert("manifests", list(&self.manifests));
        out.insert("reference", opt(&self.reference));
        out.insert("timeout_secs", num(self.timeout_secs));
        out.insert("unverifiable", flag(self.unverifiable));
        out.insert("applies_to", applies(&self.applies_to));
        out.insert("multiline", flag(self.multiline));
        out.insert("if", clause(&self.if_cond));
        out.insert("then", clause(&self.then_cond));
        out.insert("adr", opt(&self.adr));
        out.insert("owner", opt(&self.owner));
        out.insert("expiry", opt(&self.expiry));
        out.insert("effort_hours", num(self.effort_hours));
        out.insert("trigger", opt(&self.trigger));
        out.insert("evidence", opt(&self.evidence));
        out.insert("reversibility", opt(&self.reversibility));
        out.insert("covers", list(&self.covers));
        out
    }
}

fn default_severity() -> String {
    "error".into()
}

/// Дефолтный тип правила: используется только у записей `unverifiable: true`
/// (без `type`; движок их не исполняет, значение не достигает исполнения).
fn default_rule_kind() -> RuleKind {
    RuleKind::MustContain
}

/// Десериализация поля-исключения: принимает и одиночную строку, и список
/// строк (агенты и люди пишут оба варианта; `|` внутри glob НЕ
/// поддерживается — это отдельные glob'ы, а не regex-альтернатива).
fn de_string_or_list<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrList {
        /// Одиночный glob.
        One(String),
        /// Список glob'ов.
        Many(Vec<String>),
    }
    Ok(match StringOrList::deserialize(deserializer)? {
        StringOrList::One(s) => vec![s],
        StringOrList::Many(v) => v,
    })
}

/// Десериализация `deny`-списка тех-радара (волна S, ADR-070): принимает и
/// одиночную запись [`DenySpec`], и их список — форма записи (строка или
/// карта `{name, below, scopes}`) разбирается самим `DenySpec`.
fn de_deny_list<'de, D>(deserializer: D) -> std::result::Result<Vec<DenySpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DenyInput {
        /// Одиночная запись (имя или карта).
        One(DenySpec),
        /// Список записей.
        Many(Vec<DenySpec>),
    }
    Ok(match DenyInput::deserialize(deserializer)? {
        DenyInput::One(spec) => vec![spec],
        DenyInput::Many(v) => v,
    })
}

/// Исключение правила через ADR (поле верхнего уровня `overrides:`,
/// `docs/corp-spine.md`). Все три поля обязательны к заполнению: неполный
/// override — error-находка и игнорируется (гейт «только через ADR»).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OverrideEntry {
    /// Идентификатор (`id`) или имя (`name`) отключаемого правила.
    #[serde(default)]
    pub rule: Option<String>,
    /// Номер ADR, разрешающего отступление.
    #[serde(default)]
    pub adr: Option<String>,
    /// Срок действия: `YYYY-MM` (по месяцу включительно) или `YYYY-MM-DD`.
    #[serde(default)]
    pub until: Option<String>,
    /// 0.4.8+: узкое исключение — override гасит правило ТОЛЬКО на файлах,
    /// матчащихся этим глобам (та же семантика, что у `exclude_glob` правила).
    /// Пусто — правило отключается целиком (прежнее поведение). Пустой список
    /// не сериализуется: реестр без поля остаётся байт-в-байт прежним.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "de_string_or_list"
    )]
    pub exclude_glob: Vec<String>,
}

/// Статус override после оценки (отчёт `check` и `control report`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverrideInfo {
    /// Правило (как записано).
    pub rule: String,
    /// ADR (как записан, может быть пустым при неполном override).
    pub adr: String,
    /// Срок (как записан).
    pub until: String,
    /// 0.4.8+: глобы узкого override (пусто — правило отключено целиком).
    /// Пропуск пустого сохраняет прежний JSON-контракт отчёта.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_glob: Vec<String>,
    /// `active` (правило отключено) / `expired` (просрочен, правило снова
    /// действует) / `invalid` (неполный, некорректная дата или правило не
    /// найдено — игнорируется).
    pub status: String,
    /// Пояснение (текст находки).
    pub note: String,
}

/// Правило, пропущенное при разборе реестра из-за неизвестного типа (E8):
/// запись, скорее всего, из словаря другой редакции — не падение и не
/// тихий пропуск, а warn-находка `unknown_rule_type` в отчёте [`check`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedUnknownRule {
    /// Имя правила как записано (`<без имени>`, если поля `name` нет).
    pub name: String,
    /// Неизвестный тип правила (значение поля `type`).
    pub rule_type: String,
}

/// Карточка правила реестра для читателей вне `control` (детектор
/// исполняемых инвариантов, проверка зубов шаблонов, паспорт и трассировка):
/// что за правило, какого оно типа, какую команду запускает и какой инвариант
/// спайна задевает.
///
/// Поля [`FitnessRule`] крейт-видимые, но `pattern`/`command`/`ad`/`covers` —
/// приватные, поэтому внешние модули читают карточку, а не сам `FitnessRule`.
#[derive(Debug, Clone, Serialize)]
pub struct RuleCard {
    /// Идентификатор правила (`C-12`), если задан.
    pub id: Option<String>,
    /// Имя правила (код находки).
    pub name: String,
    /// Тип проверки в `snake_case` (`command_succeeds`, `must_contain`, …).
    pub kind: &'static str,
    /// Команда (только у `command_succeeds`).
    pub command: Option<String>,
    /// Задетый инвариант спайна (`ad: AD-6`), если задан.
    pub ad: Option<String>,
    /// Требования, которые правило покрывает (`covers: [...]`).
    pub covers: Vec<String>,
}

impl RuleCard {
    /// Правило проверяет ПОВЕДЕНИЕ, а не наличие текста
    /// ([`BEHAVIOUR_RULE_KINDS`]).
    #[must_use]
    pub fn is_behaviour(&self) -> bool {
        BEHAVIOUR_RULE_KINDS.contains(&self.kind)
    }

    /// Ключ правила для сопоставления с `verified_by` сущности: `id`, иначе имя.
    #[must_use]
    pub fn key(&self) -> &str {
        self.id.as_deref().unwrap_or(self.name.as_str())
    }
}

/// Типы правил, проверяющие ПОВЕДЕНИЕ (а не наличие текста): исполнение
/// команды, направление зависимостей, границы контекста, ArchUnit-гейт.
/// Используется метрикой «доля правил, проверяющих поведение» (Н10).
pub const BEHAVIOUR_RULE_KINDS: [&str; 4] = [
    "command_succeeds",
    "dependency_direction",
    "context_boundary",
    "archunit",
];

/// Нормализует severity правила: канонические `error`/`warn` и `block`/`warn`
/// корп-спайна (`docs/corp-spine.md`: block останавливает мерж, warn — только
/// в отчёт) проходят как есть; шкала кейсов и handoff-пакетов маппится:
/// `critical`/`high` → error (блокирующие), `medium` → warn (advisory).
pub(crate) fn normalize_severity(raw: &str, rule_name: &str) -> Result<&'static str> {
    match raw {
        "error" | "block" | "critical" | "high" => Ok("error"),
        "warn" | "medium" => Ok("warn"),
        other => Err(HarnessError::Control(format!(
            "правило '{rule_name}': severity должно быть error|warn|block|critical|high|medium, получено '{other}'"
        ))),
    }
}
