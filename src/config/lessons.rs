//! Конфигурация контура обучения и гейта: реестр уроков ([`LessonsConfig`],
//! ADR-085/087), маршрутизация значимости ([`SignificanceConfig`], ADR-034),
//! матрица обязательных составляющих гейта ([`GateConfig`], ADR-039) и вложенные
//! секции `[gate.*]`, `[evidence]` (ADR-041), `[trace]` (ADR-050), `[drift]`,
//! `[survey]`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

/// Реестр уроков (Lesson Ledger, ADR-085): пороги детерминированного
/// жизненного цикла урока и бюджет блока уроков в контексте.
///
/// Пороги — **единственный** источник переходов статуса (AD-2: LLM статус не
/// меняет). Дефолты воспроизводят задачу 0.5.0: `confirmed` — `hits ≥ 2` при
/// `misses = 0` на ≥ 2 разных задачах, `retired` — серия `misses ≥ 2` подряд.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LessonsConfig {
    /// Включить реестр в автоматических путях (запись из вердикта гейта, L2, и
    /// блок уроков в контексте, L3). Дефолт `false` — поведение 0.4.11; явные
    /// вызовы CLI/MCP работают независимо от флага.
    pub enabled: bool,
    /// Наблюдённых FAIL по сигнатуре до перехода в `candidate`.
    pub candidate_fails: u32,
    /// `hits`, необходимые для статуса `confirmed`.
    pub confirm_hits: u32,
    /// Потолок `misses` для статуса `confirmed` (0 — «ни одного промаха»).
    pub confirm_max_misses: u32,
    /// Число разных задач (`task`/`run_id`) для статуса `confirmed`.
    pub confirm_tasks: u32,
    /// Серия `misses` подряд, выводящая урок в `retired` (TASK §7.2: урок,
    /// который начал мешать, выводится из оборота автоматически).
    pub retire_misses: u32,
    /// Срок жизни урока в днях от даты записи (0 — без срока): истечение
    /// выводит урок в `retired` событием.
    pub expiry_days: u32,
    /// Сколько уроков отбирается в блок инъекции L3 (ADR-087).
    pub k: usize,
    /// Лимит токенов блока уроков L3 (ADR-087, оценка — символы / 4).
    pub token_limit: usize,
    /// Каталог пользовательского реестра уроков (уровни `user`/`org`).
    /// `None` — `$ARCH_HOME/lessons`; явный путь нужен раздельным контурам и
    /// изоляции тестов от реального дома (AD-7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_dir: Option<PathBuf>,
    /// Каталог организационного реестра уроков (ADR-091): общий git-репозиторий
    /// или каталог с `ledger.jsonl`. `None` — org-реестр не настроен, и
    /// `--org`/`org` (CLI/MCP) возвращает понятную ошибку, а не молчит.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_dir: Option<PathBuf>,
}

impl Default for LessonsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            candidate_fails: 2,
            confirm_hits: 2,
            confirm_max_misses: 0,
            confirm_tasks: 2,
            retire_misses: 2,
            expiry_days: 0,
            k: 5,
            token_limit: 900,
            user_dir: None,
            org_dir: None,
        }
    }
}

impl LessonsConfig {
    /// Каталог пользовательского реестра уроков (`$ARCH_HOME/lessons`, если
    /// путь не переопределён явно).
    #[must_use]
    pub fn user_ledger_dir(&self) -> PathBuf {
        self.user_dir
            .clone()
            .unwrap_or_else(crate::lessons::user_dir)
    }

    /// Путь журнала пользовательского реестра уроков.
    #[must_use]
    pub fn user_ledger_path(&self) -> PathBuf {
        self.user_ledger_dir().join(crate::lessons::LEDGER_FILE)
    }

    /// Дата истечения урока (`YYYY-MM-DD` + `expiry_days`); `None` — без срока.
    #[must_use]
    pub fn expiry_from(&self, recorded: &str) -> Option<String> {
        if self.expiry_days == 0 {
            return None;
        }
        let date = chrono::NaiveDate::parse_from_str(recorded.trim(), "%Y-%m-%d").ok()?;
        let days = i64::from(self.expiry_days);
        Some((date + chrono::Duration::days(days)).to_string())
    }
}

/// Пороги маршрутизации Architecture Significance Score (ADR-034).
///
/// Дефолты 1/4 воспроизводят историческую эвристику «0–1 → Fast, 2–4 →
/// Standard, 5+ → Critical» из `SOURCE_BRIEF` (статус — рабочая гипотеза,
/// пересмотр по данным пилота). Форсирующие critical-триггеры
/// (`security_boundary_change`, `irreversible_migration`,
/// `criticality_or_exception`) НЕ конфигурируются — fail-safe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SignificanceConfig {
    /// Верхняя граница маршрута Fast: score ≤ `fast_max` → Fast.
    pub fast_max: usize,
    /// Верхняя граница маршрута Standard: `fast_max` < score ≤ `standard_max` →
    /// Standard; выше — Critical.
    pub standard_max: usize,
    /// Глобы детектора контрактов (T-05): что считать контрактом по пути.
    #[serde(default = "default_contract_globs")]
    pub contract_globs: Vec<String>,
    /// Глобы детектора новых сущностей модели (T-05): файлы, появление
    /// которых означает новый компонент.
    #[serde(default = "default_component_globs")]
    pub component_globs: Vec<String>,
    /// Глобы детектора изменений интеграций (T-05): файлы сущностей
    /// интеграций модели.
    #[serde(default = "default_integration_globs")]
    pub integration_globs: Vec<String>,
    /// Глобы детектора NFR-сущностей (1.7 п.3): файлы NFR модели, появление
    /// или правка которых поднимает `significant_nfr`.
    #[serde(default = "default_nfr_globs")]
    pub nfr_globs: Vec<String>,
    /// Глобы границы безопасности (D2): auth-конфиги, сетевые политики, IAM —
    /// правка файла под глобом зажигает `security_boundary_change`.
    /// ПУСТ по умолчанию: глобальных эвристик нет, соглашения репозитория не
    /// зашиты в бинарь; без настройки триггер остаётся заявляемым (D1).
    #[serde(default)]
    pub security_globs: Vec<String>,
    /// Глобы зон доверия (D2): правка файла под глобом зажигает
    /// `trust_zone_change`. Пусто по умолчанию, как у `security_globs`.
    #[serde(default)]
    pub trust_zone_globs: Vec<String>,
    /// Глобы контрактов данных (D2): схемы событий/топиков — правка файла
    /// под глобом зажигает `data_contract_change`. Пусто по умолчанию.
    #[serde(default)]
    pub data_contract_globs: Vec<String>,
}

/// Дефолтные глобы контрактов (T-05): каталоги, где контракты лежат по
/// соглашению, — в дополнение к распознаванию по СОДЕРЖИМОМУ файла.
pub(crate) fn default_contract_globs() -> Vec<String> {
    vec!["docs/contracts/**".to_string(), "contracts/**".to_string()]
}

/// Дефолтный глоб новых компонентов модели (T-05).
fn default_component_globs() -> Vec<String> {
    vec!["model/CMP-*".to_string()]
}

/// Дефолтный глоб сущностей интеграций модели (T-05).
fn default_integration_globs() -> Vec<String> {
    vec!["model/INT-*".to_string()]
}

/// Дефолтный глоб NFR-сущностей модели (1.7 п.3).
fn default_nfr_globs() -> Vec<String> {
    vec!["model/NFR-*".to_string()]
}

impl Default for SignificanceConfig {
    fn default() -> Self {
        Self {
            fast_max: crate::control::DEFAULT_FAST_MAX,
            standard_max: crate::control::DEFAULT_STANDARD_MAX,
            contract_globs: default_contract_globs(),
            component_globs: default_component_globs(),
            integration_globs: default_integration_globs(),
            nfr_globs: default_nfr_globs(),
            security_globs: Vec::new(),
            trust_zone_globs: Vec::new(),
            data_contract_globs: Vec::new(),
        }
    }
}

impl SignificanceConfig {
    /// Валидированные пороги `(fast_max, standard_max)`.
    ///
    /// Валидация мягкая — не при загрузке конфига, а при чтении в
    /// `control score`/`significance_score`: `fast_max` обязан быть строго
    /// меньше `standard_max`, иначе граница Fast/Standard неопределённа.
    ///
    /// # Errors
    /// `fast_max >= standard_max` — понятный текст ошибки.
    pub fn limits(&self) -> Result<(usize, usize)> {
        if self.fast_max >= self.standard_max {
            return Err(HarnessError::Config(format!(
                "[significance]: fast_max ({}) обязан быть меньше standard_max ({}) — \
                 иначе граница маршрутов Fast/Standard неопределённа",
                self.fast_max, self.standard_max
            )));
        }
        Ok((self.fast_max, self.standard_max))
    }

    /// Глобы детекторов диффа (T-05) — из секции `[significance]`.
    #[must_use]
    pub fn diff_globs(&self) -> crate::control::DiffGlobs {
        crate::control::DiffGlobs {
            contracts: self.contract_globs.clone(),
            components: self.component_globs.clone(),
            integrations: self.integration_globs.clone(),
            nfr: self.nfr_globs.clone(),
            security: self.security_globs.clone(),
            trust_zone: self.trust_zone_globs.clone(),
            data_contract: self.data_contract_globs.clone(),
        }
    }
}

/// Матрица обязательных составляющих составного гейта по маршруту (П1 ДКА).
///
/// Имена — имена составляющих `arch-be gate`: `fitness`, `delta_guard`,
/// `rule_weakened`, `spine_lint`, `trace_check`, `sensors`, `nfr`,
/// `evidence_verify`, `decision_quality`, `semantic_quality`, `secrets`,
/// `openspec_coverage`. Пустой список = на маршруте обязательных нет.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GateConfig {
    /// Обязательные составляющие по маршрутам.
    pub required: RequiredRules,
    /// Анти-ослабление реестра (A1): severity находки `BodyChanged`.
    pub rule_weakened: RuleWeakenedConfig,
    /// Override по настоящему принятому ADR (A2): каталог ADR и горизонт.
    pub overrides: OverridesConfig,
    /// Составляющая `decision_quality` (Н7 волны B 0.3.4, ADR-042): качество
    /// архитектурных решений по отчёту рубрики-судьи.
    pub decision_quality: DecisionQualityConfig,
    /// Составляющая `semantic_quality` (ADR-052): смысловые рубрики —
    /// решение против инварианта, ссылка не на ту сущность, обещание без
    /// механизма, код против инварианта — по отчётам судьи.
    pub semantic_quality: SemanticQualityConfig,
    /// Что делать с решением рубрики `human` на каждом маршруте (E4.2).
    pub decision_policy: DecisionPolicyConfig,
    /// Составляющая `secrets` (C3 волны C 0.3.12): литеральные секреты в
    /// исходниках — детекторы [`crate::secrets::builtin_rules`].
    pub secrets: SecretsConfig,
    /// Составляющая `approval_conditions` (G, ADR-074): условия согласования
    /// решений — механическая проверка каждого условия с `check` разрешается
    /// в правило реестра `CONSTRAINTS.yaml`. Детали — [`ConditionsConfig`].
    pub conditions: ConditionsConfig,
    /// Составляющая `arch_drift` (K6 волны K 0.3.14): рёбра графа «как
    /// построено» против модели. Дорогая (снимок базовой ревизии + обход
    /// рабочего дерева), поэтому по умолчанию SKIP: включается записью
    /// `arch_drift` в `[gate.required]` маршрута (тогда находки блокируют)
    /// либо флагом здесь (находки — warn).
    pub arch_drift: ArchDriftConfig,
    /// Составляющая `openspec_coverage` (F2 волны F 0.3.14, ADR-067): покрытие
    /// требований `OpenSpec` правилами реестра — область проверки.
    #[serde(default)]
    pub openspec_coverage: OpenspecCoverageConfig,
    /// Реестр зондов доступности ресурсов `requires` (ADR-068 Am.3): секции
    /// `[gate.requires.<имя>]`. Зонд задаёт, как проверяется ресурс (файл,
    /// бинарь, переменная, hostname, устройство, команда), и текст SKIP.
    /// Переопределяет одноимённый встроенный зонд (`cuda`/`stand`); новый
    /// ресурс, не объявленный здесь и не встроенный, — ошибка реестра.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires: BTreeMap<String, crate::control::requires::RequiresProbeConfig>,
    /// Конверт вердикта (П7, ADR-043): дополнительные входы аттестации —
    /// документы решения (SOL/CTX/AGR), не входящие в закрытый список входов
    /// (секция `[gate.envelope]`).
    pub envelope: EnvelopeConfig,
}

/// Настройки конверта вердикта (П7, ADR-043): секция `[gate.envelope]`.
///
/// Решение живёт не только в реестре и модели, но и в документах решения
/// (SOL/CTX/AGR): правка такого документа обязана расходить конверт так же, как
/// правка реестра, спайна или модели. Глобы `extra` добавляют эти документы во
/// входы аттестации (вход `extra:<глоб>` — sha256 по отсортированному списку
/// файлов). Пусто (дефолт) — прежний конверт без дополнительных входов:
/// существующие кейсы аттестацию не меняют.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvelopeConfig {
    /// Глобы документов решения, чьё содержимое входит во входы конверта.
    pub extra: Vec<String>,
}

/// Настройки составляющей гейта `openspec_coverage` (F2): секция
/// `[gate.openspec_coverage]`. Только область: включение в блокирующие — через
/// `[gate.required]` маршрута (образец — `decision_quality`), ужесточений
/// без явного решения проекта нет.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenspecCoverageConfig {
    /// Область проверки (дефолт `changed` — дешёвый режим потока доработок).
    pub scope: OpenspecCoverageScope,
}

/// Область покрытия требований `OpenSpec` в составляющей гейта (F2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenspecCoverageScope {
    /// Требования дельт активных changes, затронутых диффом `base..дерево`,
    /// плюс требования живых спек, чьи файлы изменены (дефолт).
    #[default]
    Changed,
    /// Все требования — как `openspec coverage`.
    All,
}

impl OpenspecCoverageScope {
    /// Метка области для детали составляющей.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::All => "all",
        }
    }
}

/// Severity находки `BodyChanged` анти-ослабления реестра (A1).
///
/// `warn` по умолчанию: изменение тела правила — сигнал, а не приговор
/// (у `glob`/`pattern` нет формальной семантики «ужесточения», поэтому любое
/// изменение — находка, и краснить ею чужой пайплайн без явного решения
/// нельзя). Проект, готовый к строгому режиму, ставит
/// `[gate.rule_weakened] body = "error"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodySeverity {
    /// Предупреждение (дефолт): находка видна, вердикт не меняется.
    #[default]
    Warn,
    /// Блок: изменение тела правила валит составляющую `rule_weakened`.
    Error,
}

impl BodySeverity {
    /// Нормализованная метка severity (`warn`/`error`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Настройки анти-ослабления реестра (A1): секция `[gate.rule_weakened]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleWeakenedConfig {
    /// Severity находки `BodyChanged` (дефолт `warn`).
    pub body: BodySeverity,
}

/// Severity находки `secret_literal` составляющей `secrets` (C3).
///
/// `warn` по умолчанию: детекторы работают по форматам и дают ложные
/// срабатывания на длинных hex (git-хэш, sha256) — краснить ими чужой
/// пайплайн без явного решения проекта нельзя. Проект, готовый к строгому
/// режиму, ставит `[gate.secrets] severity = "error"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretSeverity {
    /// Предупреждение (дефолт): находка видна, вердикт не меняется.
    #[default]
    Warn,
    /// Блок: литеральный секрет валит составляющую `secrets`.
    Error,
}

impl SecretSeverity {
    /// Нормализованная метка severity (`warn`/`error`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Область сканирования составляющей `secrets` (C3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretScope {
    /// Только файлы диффа `base..HEAD` (как `rule_weakened`, дефолт) — дешёвый
    /// режим для потока доработок.
    #[default]
    Changed,
    /// Весь репозиторий (кроме служебных и сборочных каталогов).
    All,
}

/// Настройки составляющей `secrets` (C3): секция `[gate.secrets]`.
///
/// Сканирование исходников на литеральные секреты — эвристика известных
/// форматов, поэтому дефолт `warn`/`changed`: чужие пайплайны не краснеют,
/// включение строгости (`error`) и полной области (`all`) — явное решение.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretsConfig {
    /// Severity находки `secret_literal` (дефолт `warn`).
    pub severity: SecretSeverity,
    /// Область сканирования (дефолт `changed`).
    pub scope: SecretScope,
}

/// Severity находки `approval_condition_violated` составляющей
/// `approval_conditions` (G, ADR-074).
///
/// `warn` по умолчанию: условие согласования, чья механическая проверка
/// (`check`) не разрешается в правило реестра `CONSTRAINTS.yaml`, — сигнал
/// разрыва между обещанием решения и механикой, но краснить им чужой
/// пайплайн без явного решения проекта нельзя. Проект, готовый к строгому
/// режиму, ставит `[gate.conditions] severity = "error"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionSeverity {
    /// Предупреждение (дефолт): находка видна, вердикт не меняется.
    #[default]
    Warn,
    /// Блок: нарушенное условие валит составляющую `approval_conditions`.
    Error,
}

impl ConditionSeverity {
    /// Нормализованная метка severity (`warn`/`error`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Настройки составляющей `approval_conditions` (G, ADR-074): секция
/// `[gate.conditions]`.
///
/// Составляющая проверяет условия согласования решений (поле `conditions`
/// сущности `ADR` модели): у каждого условия с механической проверкой
/// (`check`) проверка должна разрешаться в правило реестра — иначе находка
/// `approval_condition_violated`. Условие без проверки — warn
/// `approval_condition_unchecked` + паспорт. Дефолт `warn`: чужие пайплайны
/// не краснеют, ужесточение (`error`) — явное решение проекта.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConditionsConfig {
    /// Severity находки `approval_condition_violated` (дефолт `warn`).
    pub severity: ConditionSeverity,
}

/// Настройки составляющей `arch_drift` (K6): секция `[gate.arch_drift]`.
///
/// Составляющая строит граф «как построено» дважды (базовая ревизия git +
/// рабочее дерево) и потому дорогая: по умолчанию она SKIP и включение —
/// осознанное решение проекта (обратная совместимость, правило 4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchDriftConfig {
    /// Включить `arch_drift` вне `[gate.required]`: находки о рёбрах вне
    /// модели и отклонённых рёбрах в коде — warn. `false` (дефолт) —
    /// составляющая не прогоняется, если не названа в `[gate.required]`.
    pub enabled: bool,
    /// Потолок числа читаемых в снимок файлов (ADR-068 Am.2). `None` —
    /// прежний дефолт [`crate::arch_diff::MAX_SNAPSHOT_CONTENT_FILES`] (2000).
    /// Большие кейсы (легаси с `env/` на тысячи файлов) поднимают лимит, не
    /// ломая границу bounded-работы для остальных.
    pub max_files: Option<usize>,
    /// Подстроки путей-исключений из снимка (ADR-068 Am.2): файлы, чей путь
    /// содержит подстроку, не попадают ни в список, ни в содержимое. Пусто
    /// (дефолт) — ничего не исключается.
    pub ignore: Vec<String>,
}

/// Настройки проверки overrides (A2): секция `[gate.overrides]`.
///
/// Override узаконивает ослабление правила только тогда, когда ADR с его
/// номером существует и принят, а срок `until` не уходит за горизонт. Так
/// «override на выдуманный ADR-999 с until 2099» перестаёт зеленеть гейт.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverridesConfig {
    /// Каталог ADR кейса: относительный — от корня репозитория, абсолютный —
    /// как есть. `None` — `docs/adr`.
    pub adr_dir: Option<String>,
    /// Максимальный горизонт `until`, месяцев (дефолт 12): override — срочное
    /// исключение, а не вечное отключение правила.
    pub max_horizon_months: u32,
}

impl Default for OverridesConfig {
    fn default() -> Self {
        Self {
            adr_dir: None,
            max_horizon_months: crate::control::baseline::DEFAULT_MAX_HORIZON_MONTHS,
        }
    }
}

/// Что делает гейт с решением рубрики `human` (E4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanPolicy {
    /// Предупреждение: находка видна, вердикт не меняется.
    #[default]
    Warn,
    /// Блок: составляющая уходит в SKIP, вердикт INCOMPLETE — решение за
    /// человеком (очередь `reports/human/`, E4.4).
    Human,
}

/// Политика решения `human` по маршрутам значимости (E4.2; раздел 5.2 ревью).
///
/// По умолчанию блокирует только Critical: на Fast/Standard решение модели —
/// повод для предупреждения и задачи архитектору, а не остановка потока.
/// Инъекция во входе и невалидные сэмплы этой политике **не подчиняются** —
/// они эскалируются всегда (E2/E3.2): доверие к суждению подорвано, и
/// «понизить до warn» это не настройка, а отключение страховки.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DecisionPolicyConfig {
    /// Маршрут Fast: по умолчанию предупреждение.
    pub fast: HumanPolicy,
    /// Маршрут Standard: по умолчанию предупреждение и задача архитектору.
    pub standard: HumanPolicy,
    /// Маршрут Critical: по умолчанию блок до решения архитектора.
    pub critical: HumanPolicy,
}

impl Default for DecisionPolicyConfig {
    fn default() -> Self {
        Self {
            fast: HumanPolicy::Warn,
            standard: HumanPolicy::Warn,
            critical: HumanPolicy::Human,
        }
    }
}

impl DecisionPolicyConfig {
    /// Политика маршрута; библиотечный вызов без маршрута — как на Fast
    /// (мягко): эскалация требует явного решения проекта.
    #[must_use]
    pub fn for_route(&self, route: Option<crate::control::Route>) -> HumanPolicy {
        match route {
            Some(crate::control::Route::Critical) => self.critical,
            Some(crate::control::Route::Standard) => self.standard,
            Some(crate::control::Route::Fast) | None => self.fast,
        }
    }
}

/// На что смотрит составляющая `semantic_quality` (ADR-052).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticScope {
    /// Субъекты, чьё досье затронуто диффом от базы или активной дельтой, —
    /// дешёвый режим для потока доработок (дефолт).
    #[default]
    Changed,
    /// Весь пакет: все ADR, сущности модели и файлы кода под корнями компонент.
    All,
}

/// Составляющая `semantic_quality` (ADR-052).
///
/// Пустой список `rubrics` — составляющая ничего не требует, даже будучи
/// включённой в `[gate.required]`: включать её можно раньше, чем заводить
/// отчёты. Оценка дорогая (k сэмплов судьи на субъект), поэтому гейт **не
/// оценивает** — он читает отчёты, которые положил хост, и переиспользует
/// отчёт с совпавшим хэшем досье (ADR-051).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticQualityConfig {
    /// Имена рубрик из каталога рубрик (`paths.rubrics_dir`), обязательных к
    /// отчёту. Пусто — ничего не требуется.
    #[serde(default)]
    pub rubrics: Vec<String>,
    /// Область субъектов.
    #[serde(default)]
    pub scope: SemanticScope,
    /// Минимальный взвешенный итог отчёта.
    #[serde(default = "default_semantic_min_score")]
    pub min_score: f64,
    /// Требовать от судьи модель, отличную от автора документа: `true` —
    /// `judge_is_author` становится error, `false` — warn.
    #[serde(default)]
    pub require_distinct_judge: bool,
    /// Порог доли невалидных сэмплов (балл вне шкалы) для смысловых рубрик
    /// (E3.2): выше порога составляющая уходит в SKIP — вердикт INCOMPLETE.
    #[serde(default = "default_max_invalid_ratio")]
    pub max_invalid_samples_ratio: f64,
    /// Требовать от судьи пройденную квалификацию (E6.3): на блокирующем
    /// маршруте находка `judge_unqualified` → INCOMPLETE. Дефолт `false`.
    #[serde(default)]
    pub require_qualified_judge: bool,
}

/// Дефолт порога доли невалидных сэмплов (E3.2): половина и больше.
fn default_max_invalid_ratio() -> f64 {
    0.5
}

/// Порог взвешенного итога смысловой рубрики по умолчанию.
///
/// `3.0`, а не `3.5` как у `decision_quality`: порог смысловой рубрики
/// откалиброван по живому прогону 2026-09-20, а не по аналогии. На четырёх
/// прогонах `adr_spine_consistency`/`model_link_semantics`/
/// `code_invariant_conformance` чистый субъект дал 3.43, засеянные дефекты —
/// 2.12, 2.00 и 1.00. При `3.5` краснел бы единственный чистый контроль
/// (3.43 < 3.5), то есть порог был бы заведомо ложным; `3.0` разделяет те же
/// четыре точки с запасом 0.43 снизу и 0.88 сверху. Замер — в
/// `docs/experiments/semantic-rubrics-2026-09-20.md`; порог настраивается
/// (`min_score`), потому что выборка мала.
fn default_semantic_min_score() -> f64 {
    3.0
}

impl Default for SemanticQualityConfig {
    fn default() -> Self {
        Self {
            rubrics: Vec::new(),
            scope: SemanticScope::Changed,
            min_score: default_semantic_min_score(),
            require_distinct_judge: false,
            max_invalid_samples_ratio: default_max_invalid_ratio(),
            require_qualified_judge: false,
        }
    }
}

/// Настройки составляющей гейта `decision_quality` (Н7, ADR-042).
///
/// Составляющая по умолчанию **не обязательна** ни на одном маршруте: SKIP
/// обязательной составляющей даёт INCOMPLETE, и включение порога качества
/// должно быть осознанным решением проекта, а не сюрпризом после обновления.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DecisionQualityConfig {
    /// Минимальный взвешенный итог рубрики `adr_quality` для Accepted-ADR.
    /// Порог `3.5` — как в чек-листе скилла `adr-authoring`.
    pub min_score: f64,
    /// Требовать от судьи модель, отличную от автора документа: `true` —
    /// `judge_is_author` становится error, `false` — warn.
    pub require_distinct_judge: bool,
    /// Требовать судью из другого СЕМЕЙСТВА моделей (ADR-048): `true` —
    /// `judge_same_family` становится error, `false` (дефолт) — warn.
    /// По умолчанию warn: смена модели внутри одного семейства — обычная
    /// практика, а не нарушение; ужесточение — осознанный выбор проекта.
    pub require_distinct_family: bool,
    /// Минимальный уровень независимости судьи (ADR-048): ниже порога —
    /// находка `judge_independence_low` (error). Дефолт `none` — поведение
    /// 0.3.4: независимость заявлена, порога нет. Уровни:
    /// `none` < `declared` < `declared_cross_family` < `launched` <
    /// `launched_cross_family`.
    pub min_independence: String,
    /// Порог доли сэмплов судьи с баллом вне шкалы (E3.2). Выше порога
    /// суждению верить нельзя: составляющая уходит в SKIP — вердикт
    /// INCOMPLETE, решение за человеком. Дефолт `0.5`: половина сэмплов и
    /// больше — не «шум одного ответа». Любая ненулевая доля ниже порога
    /// оставляет warn-находку, поэтому невалидный сэмпл не бывает невидимым.
    pub max_invalid_samples_ratio: f64,
    /// Требовать от судьи пройденную квалификацию на эталонном наборе (E6.3):
    /// на блокирующем маршруте находка `judge_unqualified` → INCOMPLETE.
    /// Дефолт `false` — поведение 0.3.8: допуск включает проект осознанно,
    /// прогнав `arch-be rubric qualify` (иначе ужесточение покраснило бы
    /// чужие пайплайны без предупреждения).
    #[serde(default)]
    pub require_qualified_judge: bool,
}

impl Default for DecisionQualityConfig {
    fn default() -> Self {
        Self {
            min_score: 3.5,
            require_distinct_judge: false,
            require_distinct_family: false,
            min_independence: crate::judge::INDEPENDENCE_NONE.to_string(),
            max_invalid_samples_ratio: 0.5,
            require_qualified_judge: false,
        }
    }
}

/// Семантика артефактов Evidence Bundle (Н1 волны A 0.3.4, ADR-041).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceSemantics {
    /// По маршруту: Critical — `error`, Standard/Fast — `warn` (дефолт).
    #[default]
    Auto,
    /// Проверки содержания выключены (поведение 0.3.3).
    Off,
    /// Все находки о содержании — предупреждения, выпуск не блокируют.
    Warn,
    /// Все находки о содержании блокируют выпуск на любом маршруте.
    Error,
}

/// Секция `[evidence]`: семантика артефактов бандла (Н1, ADR-041).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EvidenceConfig {
    /// Минимальный размер артефакта в байтах: меньше — «пустышка».
    pub min_bytes: u64,
    /// Строгость находок о содержании артефактов.
    pub semantics: EvidenceSemantics,
    /// Требование машинных записей прогонов (A1, 0.3.14): `true` — отчёт
    /// `validation`/`fitness_report`/`walking_skeleton`, написанный прозой без
    /// записи `.arch-handoff/evidence/<kind>.json` (`arch-be evidence record`),
    /// блокирует выпуск (`evidence_report_unbound` — `error`); `false`
    /// (дефолт) — та же находка остаётся предупреждением. Схема «warn → error
    /// по флагу» (правило 4): поведение прежних версий не меняется без явного
    /// решения проекта.
    #[serde(default)]
    pub require_records: bool,
}

/// Дефолтный порог «пустышки»: 200 байт (стартовое предложение ТЗ 0.3.4,
/// обоснование — ADR-041).
pub const DEFAULT_EVIDENCE_MIN_BYTES: u64 = 200;

impl Default for EvidenceConfig {
    fn default() -> Self {
        Self {
            min_bytes: DEFAULT_EVIDENCE_MIN_BYTES,
            semantics: EvidenceSemantics::Auto,
            require_records: false,
        }
    }
}

impl EvidenceConfig {
    /// Строгость находок о содержании для маршрута (`Auto` разворачивается
    /// по маршруту: Critical — блокирует выпуск).
    #[must_use]
    pub fn severity_for(&self, route: crate::control::Route) -> Option<&'static str> {
        match self.semantics {
            EvidenceSemantics::Off => None,
            EvidenceSemantics::Warn => Some("warn"),
            EvidenceSemantics::Error => Some("error"),
            EvidenceSemantics::Auto => Some(if route == crate::control::Route::Critical {
                "error"
            } else {
                "warn"
            }),
        }
    }
}

/// Строгость требования «у инварианта есть исполняемая проверка» (ADR-050).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutableRequired {
    /// Требования нет: звено «AD → исполняемое правило» информационное (дефолт).
    #[default]
    Off,
    /// Находка `ad-text-only` уровня warn: сигнал, вердикт не меняется.
    Warn,
    /// Находка `ad-text-only` уровня error: гейт краснеет.
    Error,
}

/// Секция `[trace]`: требование исполняемой проверки инвариантов (ADR-050).
///
/// Дефолт `off` — обратная совместимость: до 0.3.5 находки `ad-text-only` не
/// существовало, и включение требования — осознанное решение проекта.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceConfig {
    /// Строгость находки `ad-text-only` по инвариантам без исполняемой проверки.
    pub executable_required: ExecutableRequired,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            executable_required: ExecutableRequired::Off,
        }
    }
}

/// Секция `[drift]`: проверки дрейфа «модель ↔ код» (волна C 0.3.14).
///
/// Дефолт off — обратная совместимость: находка `nfr-metric-missing`
/// появляется только при осознанном включении (`[drift] nfr_metric_check =
/// true` в конфиге кейса).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DriftConfig {
    /// C3: метрика из `verification` NFR (`snake_case` с суффиксом
    /// `_seconds`/`_total`/`_bytes`/`_ratio`) обязана встречаться в коде CMP,
    /// реализующих NFR (`implements`), иначе warn-находка
    /// `nfr-metric-missing`.
    pub nfr_metric_check: bool,
}

/// Секция `[survey]`: денилист обследования (B8 волны B).
///
/// Списки **дополняют** встроенный набор (lock-файлы, лицензии, метаданные
/// чартов, funding-хосты), а не заменяют его: пользователь может добавить
/// свои пути/хосты, не теряя защиты по умолчанию. Совпадение пути — по
/// имени файла или glob-суффиксу (`*.lock`); совпадение хоста — по
/// равенству или dot-суффиксу (`example.com` покрывает `www.example.com`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SurveyConfig {
    /// Дополнительные хосты, которые не считаются интеграциями.
    pub ignore_hosts: Vec<String>,
    /// Дополнительные пути/имена файлов, которые не сканируются как
    /// источники интеграций.
    pub ignore_paths: Vec<String>,
}

impl ExecutableRequired {
    /// Severity находки `ad-text-only` (`None` — находки нет).
    #[must_use]
    pub fn severity(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Warn => Some("warn"),
            Self::Error => Some("error"),
        }
    }
}

/// Списки обязательных составляющих для Fast / Standard / Critical.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RequiredRules {
    /// Маршрут Fast.
    pub fast: Vec<String>,
    /// Маршрут Standard.
    pub standard: Vec<String>,
    /// Маршрут Critical.
    pub critical: Vec<String>,
}

impl Default for RequiredRules {
    fn default() -> Self {
        let base = vec!["fitness".to_string(), "spine_lint".to_string()];
        let mut standard = base.clone();
        standard.extend(
            ["trace_check", "rule_weakened", "nfr", "model_validate"]
                .iter()
                .map(|s| (*s).to_string()),
        );
        let mut critical = standard.clone();
        critical.extend(
            ["delta_guard", "sensors", "evidence_verify"]
                .iter()
                .map(|s| (*s).to_string()),
        );
        Self {
            fast: base,
            standard,
            critical,
        }
    }
}
