//! CLI-слой бинаря `arch-be`: clap-структуры команд, диспетчеризация
//! подкоманд и общие хелперы CLI-края (B1: выделено из `main.rs`;
//! обработчики групп подкоманд — в подмодулях `cli::*`).

mod agent;
mod archify;
mod archunit;
mod automation;
mod bench;
mod control;
mod fleet;
mod library;
mod mcp;
mod misc;
mod model;
mod rubric;
mod rules;
mod specs;
mod web;

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use arch_harness::config::Config;
use arch_harness::llm::LlmRegistry;

#[cfg(feature = "harness")]
use agent::{RunOptions, cmd_run};
use archify::{ArchifyCmd, cmd_archify};
use archunit::{ArchunitCmd, cmd_archunit};
#[cfg(feature = "harness")]
use automation::{CronCmd, EvalCmd, cmd_cron, cmd_eval};
#[cfg(feature = "harness")]
use bench::{BenchCmd, cmd_bench};
use control::{ControlCmd, cmd_control};
use fleet::{FleetCmd, cmd_fleet};
#[cfg(feature = "harness")]
use fleet::{WorktreeCmd, cmd_worktree};
#[cfg(feature = "harness")]
use library::cmd_prompts;
use library::{MemoryCmd, PluginsCmd, SkillsCmd, cmd_init, cmd_memory, cmd_plugins, cmd_skills};
use mcp::{McpCmd, cmd_mcp};
use misc::{cmd_policy, cmd_survey};
use model::{ModelCmd, NfrCmd, TraceCmd, cmd_model, cmd_nfr, cmd_trace};
use rubric::{RubricCmd, cmd_rubric};
use rules::{RulesCmd, cmd_rules};
use specs::{
    AdrCmd, AgentsMdCmd, DeltaCmd, EvidenceCmd, OpenspecCmd, PublishCmd, cmd_adr, cmd_agents_md,
    cmd_delta, cmd_evidence, cmd_openspec, cmd_publish,
};
#[cfg(feature = "harness")]
use web::{WebCmd, cmd_web};

/// Доменный харнесс solution-архитектора.
#[derive(Parser)]
#[command(name = "arch-be", version, about, long_about = None)]
struct Cli {
    /// Путь к config.toml (иначе ./arch-harness.toml или ~/.config/arch-harness/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Интерактивный TUI (действие по умолчанию; только сборка `harness`).
    #[cfg(feature = "harness")]
    Tui,
    /// Инициализация ~/.arch-harness: конфиг, ассеты, примеры.
    Init,
    /// Headless-прогон агента: `arch-be run "задача"` или `cat spec.md | arch-be run -`.
    /// Только сборка `harness` (агентный цикл).
    #[cfg(feature = "harness")]
    Run {
        /// Промпт; `-` или отсутствие значения при пайпе — читать stdin.
        prompt: Option<String>,
        /// Модель (имя из [models]).
        #[arg(long)]
        model: Option<String>,
        /// Без стриминга (печатать только финальный ответ).
        #[arg(long)]
        no_stream: bool,
        /// Строгий headless-контракт (как `dsh --profile headless`):
        /// stdout — только финальный ответ ассистента, прогресс молчит;
        /// при успехе stderr пуст, при сбое — причина в stderr и exit 1.
        /// Для скриптов и пайпов: `arch-be run -q "…" > answer.md`.
        #[arg(long, short = 'q')]
        quiet: bool,
        /// Общий таймаут прогона в секундах: по истечении — причина в stderr
        /// и exit 1 (страховка CI/cron от зависшего провайдера).
        #[arg(long, value_name = "SECS")]
        timeout: Option<u64>,
        /// Лимит итераций инструментов на этот прогон (перекрывает
        /// `agent.max_tool_turns` из конфига).
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
        max_turns: Option<u64>,
        /// Ризонинг-режим: on|off (в запросы сливается карта `thinking_on/off`
        /// из конфига модели; без флага — дефолт провайдера).
        #[arg(long, value_name = "on|off")]
        think: Option<String>,
    },
    /// Список настроенных моделей.
    Models,
    /// Библиотека промптов: список или показ шаблона.
    /// Только сборка `harness` (библиотека живёт в модуле агентного цикла).
    #[cfg(feature = "harness")]
    Prompts {
        /// Имя шаблона (без — список).
        name: Option<String>,
    },
    /// Глобальная md-память (MEMORY.md): показать путь и содержимое.
    Memory {
        #[command(subcommand)]
        cmd: Option<MemoryCmd>,
    },
    /// Рендер mermaid-файла в Unicode/ASCII-арт.
    Mermaid {
        /// Файл с диаграммой (`-` — stdin).
        file: String,
    },
    /// Archify: валидация, доставка и сравнение диаграмм (JSON IR → HTML/SVG).
    Archify {
        #[command(subcommand)]
        cmd: ArchifyCmd,
    },
    /// Рубрики архитектурного контроля.
    Rubric {
        #[command(subcommand)]
        cmd: RubricCmd,
    },
    /// Правила архитектурного контроля: кандидаты и шаблоны исполняемых правил.
    Rules {
        #[command(subcommand)]
        cmd: RulesCmd,
    },
    /// Архитектурные бенчмарки. Только сборка `harness`.
    #[cfg(feature = "harness")]
    Bench {
        #[command(subcommand)]
        cmd: BenchCmd,
    },
    /// Поиск по локальной базе знаний.
    Kb {
        /// Запрос.
        query: String,
        /// Максимум результатов.
        #[arg(long, default_value_t = 8)]
        limit: usize,
    },
    /// Веб: поиск и фетч по архитектурным сайтам. Только сборка `harness`
    /// (сетевой стек reqwest + scraper).
    #[cfg(feature = "harness")]
    Web {
        #[command(subcommand)]
        cmd: WebCmd,
    },
    /// MCP-серверы: список и вызовы.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Сформировать handoff-пакет для кодового харнесса.
    /// Только сборка `harness` (адаптеры кодовых харнессов).
    #[cfg(feature = "harness")]
    Handoff {
        /// Имя харнесса (claude-code, qwen-code, openclaw, hermes, theseus, codewhale, kimi-code).
        harness: String,
        /// Путь к репозиторию.
        #[arg(long)]
        repo: PathBuf,
        /// Формулировка задачи.
        #[arg(long)]
        task: String,
        /// Файлы спек/спайна/ADR для включения.
        #[arg(long)]
        spec: Vec<PathBuf>,
        /// Явный план отката (иначе — откат на baseline-коммит).
        #[arg(long)]
        rollback: Option<String>,
        /// Маршрут значимости: fast|standard|critical (таймаут прогона: 1800/3600/7200 с).
        #[arg(long, default_value = "standard")]
        route: String,
        /// Перезаписать существующий пакетный CONSTRAINTS.yaml (T-02):
        /// без флага правки архитектора в пакете сохраняются.
        #[arg(long)]
        refresh_constraints: bool,
    },
    /// Прогнать кодовый харнесс по handoff-пакету. Только сборка `harness`.
    #[cfg(feature = "harness")]
    HarnessRun {
        /// Имя харнесса.
        harness: String,
        /// Путь к репозиторию (с .arch-handoff/).
        #[arg(long)]
        repo: PathBuf,
        /// Задача (иначе — из .arch-handoff/TASK.md).
        #[arg(long)]
        task: Option<String>,
    },
    /// Список известных кодовых харнессов. Только сборка `harness`.
    #[cfg(feature = "harness")]
    Harnesses,
    /// Архитектурный контроль.
    Control {
        #[command(subcommand)]
        cmd: ControlCmd,
    },
    /// Единый архитектурный гейт репозитория: fitness (control check) +
    /// гейт прямых правок спайна (delta guard) + анти-ослабление правил
    /// (`rule_weakened`) + линтер спайна + трассировка; на маршрутах
    /// Standard/Critical — количественные NFR и проверка evidence-бандлов.
    /// Провал любой составляющей — exit 1 (механически, без разбора строк).
    Gate {
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Маршрут: auto (механически из git-диффа, дефолт) | fast |
        /// standard | critical. Явное значение переопределяет авто-режим.
        #[arg(long, default_value = "auto")]
        route: String,
        /// База git для диффа и сравнения правил (по умолчанию — рабочее
        /// дерево против HEAD; голая ревизия — напр. origin/main — или
        /// готовый диапазон origin/main...HEAD).
        #[arg(long)]
        base: Option<String>,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Формат вывода: text (дефолт) | json (конверт вердикта с
        /// аттестацией) | sarif | junit | gitlab-codequality | markdown.
        /// Машинные форматы — строго в stdout (артефакт CI), exit-код не
        /// меняется (красный гейт — данные отчёта: 1; INCOMPLETE: 3).
        #[arg(long, default_value = "text", value_name = "FORMAT")]
        format: String,
        /// Сверить ранее сохранённый конверт вердикта с текущим состоянием
        /// дерева (Н3, ADR-043): «вердикт относится к этому состоянию» либо
        /// «состояние изменилось: <что>». Exit 1, если состояние разошлось.
        #[arg(long, value_name = "FILE")]
        verify_envelope: Option<PathBuf>,
        /// Паспорт вердикта (W1): одна страница markdown — что механика
        /// проверила, что заявлено, но ею не проверяется, и что не проверено.
        /// Заменяет обычный вывод; с `--format json` добавляет ключ `passport`
        /// в конверт вердикта (exit-код и состав конверта не меняются).
        #[arg(long)]
        explain: bool,
        /// НЕ исполнять правила `command_succeeds` (модель доверия A3,
        /// ADR-053): составляющая `fitness` уходит в SKIP `command_untrusted`
        /// — для гейта на чужом репозитории (PR из форка). То же делает
        /// `ARCH_NO_EXEC=1`; приоритет над allow-файлом `rules allow`.
        #[arg(long)]
        no_exec: bool,
    },
    /// Метрика доверия к контуру (W4): положение на шкале 1–5 с ЯКОРЯМИ и
    /// ДОКАЗАТЕЛЬСТВАМИ — какие якоря выполнены, какие нет и почему. Источники:
    /// журнал MCP-вызовов, реестр правил и регистр FP, результат `redteam
    /// --save`, вердикт гейта и отчёты рубрик. Ничего не блокирует: отвечает,
    /// насколько можно верить зелёному этого контура.
    Trust {
        /// Репозиторий или кейс (по умолчанию — текущий каталог).
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Формат: text (дефолт) | json.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Первый зелёный за 15 минут (W3): создать каркас кейса и назвать
    /// следующую красную находку с подсказкой — «дорожка до зелёного».
    /// Каркас намеренно красный: его заглушки ловит семантика бандла (Н1),
    /// иначе проводник производил бы ложнозелёные пакеты. Ничего не решает
    /// за человека: A3 не подписывает, решение не пишет, составляющие не
    /// включает.
    Bootstrap {
        /// Человеческое имя кейса (идёт в титулы: «Зарплатные и социальные
        /// выплаты»). Для `--status` необязательно, если задан `--dir`.
        name: Option<String>,
        /// Каталог кейса (по умолчанию — транслит имени).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Домен кейса: подставляется в тексты каркаса (payments, …).
        #[arg(long, default_value = "payments")]
        domain: String,
        /// Показать прогресс существующего кейса и следующий шаг, ничего не
        /// создавая: `спайн ✓ · правила ✓ · модель ✗ (2 находки) · бандл 7/13`.
        #[arg(long)]
        status: bool,
    },
    /// Метаморфный самотест вердикта (П8 ДКА): свойства ответов гейта на
    /// изолированной песочнице — монотонность по маршруту, достижимость
    /// зелёного, чувствительность к засеянному дефекту, храповик ROUTE.lock,
    /// идемпотентность, инвариантность к написанию пути. Ненулевой exit —
    /// свойство нарушено.
    Selftest {
        /// Машиночитаемый вывод: JSON-отчёт.
        #[arg(long)]
        json: bool,
        /// Расширенный режим: мутационный прогон по кейсу (`--redteam <кейс>`)
        /// наравне с метаморфными инвариантами.
        #[arg(long, value_name = "DIR")]
        redteam: Option<PathBuf>,
        /// Порог доли обнаружения для `--redteam`, % (дефолт 78 ≈ 11 из 14).
        #[arg(long, default_value_t = 78.0)]
        min_detection: f64,
    },
    /// Мутационное тестирование архитектурного пакета: клонирует кейс во
    /// временный каталог, засеивает по одному дефекту из red-team набора,
    /// гоняет гейт и печатает карту обнаружения. Read-only к исходному кейсу,
    /// без сети, детерминированно; exit 1, если доля ниже `--min-detection`.
    Redteam {
        /// Кейс (каталог с model/, CONSTRAINTS.yaml, docs/adr, бандлом…);
        /// не нужен при подкоманде `semantic-score`.
        case: Option<PathBuf>,
        /// Подкоманда: смысловая строка по сохранённым клонам (ADR-051).
        #[command(subcommand)]
        cmd: Option<RedteamCmd>,
        /// Формат вывода: text (дефолт, карта обнаружения) | json | markdown.
        #[arg(long, default_value = "text", value_name = "FORMAT")]
        format: String,
        /// Порог доли обнаружения, ниже которого exit 1 (дефолт 0.78).
        #[arg(long, default_value_t = 0.78)]
        min_detection: f64,
        /// Не включать необязательную составляющую `decision_quality`
        /// (по умолчанию прогон её включает: иначе дефект D9 не проверяем).
        #[arg(long)]
        no_decision_quality: bool,
        /// Сохранить итог измерения в `.arch-handoff/redteam.json` кейса —
        /// его читает метрика доверия (`arch-be trust`): доля обнаружения
        /// должна быть измерена, а не пересказана.
        #[arg(long)]
        save: bool,
        /// Сохранить клоны смысловых мутантов в каталог и положить рядом
        /// `SEMANTIC-TODO.json` (ADR-051): судит их модель хоста, а не
        /// харнесс, — в ядре LLM нет. В долю обнаружения они не входят.
        #[arg(long, value_name = "КАТАЛОГ")]
        keep_semantic: Option<PathBuf>,
    },
    /// Составное архитектурное ревью репозитория одним ответом (бэклог
    /// волны 3, п.13): маршрут значимости из git-диффа + весь контур
    /// единого гейта (fitness, delta guard, анти-ослабление правил, линтер
    /// спайна, трассировка; на Standard/Critical — NFR и evidence) +
    /// целостность модели + линт контрактов OpenAPI/AsyncAPI.
    /// Провал любой секции — exit 1 (механически, как у `gate`).
    Review {
        /// Репозиторий.
        dir: PathBuf,
        /// База git для диффа и сравнения правил (по умолчанию — рабочее
        /// дерево против HEAD; голая ревизия или готовый диапазон A...HEAD).
        #[arg(long)]
        base: Option<String>,
        /// Файл ограничений (по умолчанию <dir>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Машиночитаемый вывод: JSON-отчёт (passed + секции + находки).
        #[arg(long)]
        json: bool,
        /// НЕ исполнять правила `command_succeeds` (модель доверия A3,
        /// ADR-053): секция `fitness` уходит в SKIP `command_untrusted` — для
        /// ревью чужого репозитория. То же делает `ARCH_NO_EXEC=1`.
        #[arg(long)]
        no_exec: bool,
    },
    /// Дифф двух версий контракта на ломающие изменения (бэклог волны 3,
    /// п.14): `OpenAPI` 3.x (CD-001..CD-007), protobuf/gRPC (.proto),
    /// Avro (.avsc), JSON Schema топиков, DDL-миграции (.sql). Ломающее
    /// изменение — exit 1 (гейт для CI). С `--model` (корень кейса с
    /// model/) ломающий дифф сразу возвращает потребителей и владельцев
    /// по полю `contract` у INT (ADR-035). Форматы вывода для CI —
    /// `--format sarif|junit|gitlab-codequality|markdown` (волна 2, п.8).
    #[command(name = "contract-diff", visible_alias = "contract_diff")]
    ContractDiff {
        /// Старая версия контракта (yaml/yml/json/proto/avsc/sql).
        old: PathBuf,
        /// Новая версия контракта (тот же формат).
        new: PathBuf,
        /// Язык контракта: auto (детектор, дефолт) | openapi | proto | avro |
        /// jsonschema | ddl.
        #[arg(long, default_value = "auto")]
        contract_format: String,
        /// Формат вывода для CI: text (дефолт) | sarif | junit |
        /// gitlab-codequality | markdown (машинные — в stdout, как --json;
        /// несовместим с --json).
        #[arg(
            long,
            default_value = "text",
            value_name = "FORMAT",
            conflicts_with = "json"
        )]
        format: String,
        /// Корень кейса с model/ — секция impact (потребители/владельцы
        /// ломаемого контракта, ADR-035).
        #[arg(long)]
        model: Option<PathBuf>,
        /// Машиночитаемый вывод: JSON-отчёт (passed + findings + impact).
        #[arg(long)]
        json: bool,
    },
    /// Реестр ADR: глобальная агрегация решений по набору проектов (ADR-036).
    Adr {
        #[command(subcommand)]
        cmd: AdrCmd,
    },
    /// Публикация артефактов в корпоративные системы (файловые адаптеры,
    /// ADR-033: git и файлы — транспорт, живых коннекторов нет).
    Publish {
        #[command(subcommand)]
        cmd: PublishCmd,
    },
    /// Типизированная модель архитектуры (каталог model/, ADR-003).
    Model {
        #[command(subcommand)]
        cmd: ModelCmd,
    },
    /// Трассируемость модели как fitness-функция (ADR-006).
    Trace {
        #[command(subcommand)]
        cmd: TraceCmd,
    },
    /// Количественные NFR поверх модели: latency-бюджет, доступность,
    /// ёмкость, стоимость (ADR-007).
    Nfr {
        #[command(subcommand)]
        cmd: NfrCmd,
    },
    /// Библиотека скиллов: список, поиск, показ.
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
    /// Плагины (скиллы + MCP в одном пакете).
    Plugins {
        #[command(subcommand)]
        cmd: PluginsCmd,
    },
    /// Политика автономии (R-уровни): показать/проверить класс риска команды.
    Policy {
        /// Проверить команду: как её классифицирует политика.
        #[arg(long)]
        check: Option<String>,
    },
    /// Evidence Bundle — аудиторский след как условие выпуска.
    Evidence {
        #[command(subcommand)]
        cmd: EvidenceCmd,
    },
    /// Операционные метрики харнесса (из журналов сессий и отчётов).
    Metrics {
        /// Смета по реальному usage: таблица по моделям, топ-10 дорогих сессий.
        #[arg(long)]
        cost_report: bool,
    },
    /// Недельный дайджест outcome-данных MCP-контроля (`docs/outcome-metrics.md`):
    /// итерации FAIL→PASS по инструментам, топ нарушаемых правил, доля ложных
    /// срабатываний (регистр `evidence/fp-register.md`), истекающие overrides
    /// и expiry правил. Источник — журнал `.arch-handoff/mcp-calls.jsonl`.
    Digest {
        /// Репозиторий проекта (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Недельное окно (дефолт; синоним `--days 7`).
        #[arg(long)]
        week: bool,
        /// Окно в днях (перекрывает дефолтную неделю).
        #[arg(long, value_name = "N")]
        days: Option<u32>,
        /// Машиночитаемый вывод: JSON-отчёт `DigestReport`.
        #[arg(long)]
        json: bool,
    },
    /// Диагностика окружения: ключи, каталоги, плагины, харнессы, MCP.
    /// С `--host <хост>` — точечная проверка подключения `connect <host>`:
    /// бинарь arch-be в PATH, файл настроек хоста с `mcpServers.spine`,
    /// скиллы на месте, версия хоста.
    Doctor {
        /// Хост connect: claude | qwen | gigacode | codex | kimi | omp | generic.
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        /// Каталог проекта для проверки хоста (по умолчанию — текущий).
        #[arg(long, requires = "host")]
        dir: Option<PathBuf>,
    },
    /// Экспорт журнала сессии в Word/Excel.
    Export {
        /// Формат: word (docx) или excel (xlsx).
        format: String,
        /// Путь к журналу сессии (session-*.jsonl).
        session: PathBuf,
        /// Куда писать файл (.docx/.xlsx).
        out: PathBuf,
    },
    /// Дельта-спецификации (propose → apply → archive).
    Delta {
        #[command(subcommand)]
        cmd: DeltaCmd,
    },
    /// Адаптер `OpenSpec`: требования openspec/ → покрытие fitness-правилами
    /// (MVP, `docs/openspec.md`).
    Openspec {
        #[command(subcommand)]
        cmd: OpenspecCmd,
    },
    /// AGENTS.md для репозиториев команд: генерация из архитектурных артефактов.
    AgentsMd {
        #[command(subcommand)]
        cmd: AgentsMdCmd,
    },
    /// Планировщик md-задач. Только сборка `harness`.
    #[cfg(feature = "harness")]
    Cron {
        #[command(subcommand)]
        cmd: CronCmd,
    },
    /// Регрессионные eval-сьюты конфигурации харнесса (continuous evals).
    /// Только сборка `harness`.
    #[cfg(feature = "harness")]
    Eval {
        #[command(subcommand)]
        cmd: EvalCmd,
    },
    /// Worktree-фабрика: изоляция агентной работы в git worktree (review/accept/drop).
    /// Только сборка `harness`.
    #[cfg(feature = "harness")]
    Worktree {
        #[command(subcommand)]
        cmd: WorktreeCmd,
    },
    /// Аудит флота worktree: дубли и дрейф копий спайна (модель 5.2, SSOT).
    Fleet {
        #[command(subcommand)]
        cmd: FleetCmd,
    },
    /// Обратное обследование legacy-репозитория (reverse discovery):
    /// детерминированный сканер → каркас карты обследования docs/reverse/survey.md.
    Survey {
        /// Репозиторий для обследования.
        repo: PathBuf,
        /// Каталог вывода (по умолчанию <repo>/docs/reverse).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// `ArchUnit`-мост: JVM-гейты из `CONSTRAINTS.yaml` настоящим `ArchUnit`
    /// (ADR-039): генерация `JUnit`-теста, standalone-гейт, загрузка jar'ов.
    Archunit {
        #[command(subcommand)]
        cmd: ArchunitCmd,
    },
    /// Подключить Spine к внешнему CLI-агенту (MCP-сервер + скиллы + хуки):
    /// claude | qwen | gigacode | codex | kimi | omp | generic. Особые значения —
    /// гейты, не зависящие от хоста: `ci` (джоба архитектурного гейта под
    /// `--provider gitlab|github|jenkins`) и `git-hooks` (pre-commit + pre-push).
    /// (Инверсия харнесса, шаг 3; называется `connect`, т.к. `export` занят
    /// экспортом журнала.)
    Connect {
        /// Хост: claude | qwen | gigacode | codex | kimi | omp | generic |
        /// ci | git-hooks.
        host: String,
        /// CI-провайдер (только для `connect ci`): gitlab | github | jenkins.
        #[arg(long, value_name = "PROVIDER")]
        provider: Option<String>,
        /// Каталог проекта (по умолчанию — текущий).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Открыть rw-контур MCP-сервера (`arch-be mcp serve --rw`):
        /// аддитивные записи в рабочий каталог клиента — `adr_new`,
        ///   `agentsmd_generate`, `archify_compare`, `archify_deliver`,
        ///   `archify_show`, `delta_propose`, `evidence_pack`, `handoff_create`,
        ///   `reverse_survey`, `rule_template_apply`, `skill_distill`.
        ///
        /// `--rw=reports` — узкий режим для судейского харнесса: запись
        /// разрешена только отчётам рубрики, остальной белый список закрыт.
        #[arg(long, value_name = "full|reports", num_args = 0..=1, default_missing_value = "full")]
        rw: Option<String>,
        /// Не раскладывать скиллы.
        #[arg(long)]
        no_skills: bool,
        /// Не встраивать хуки.
        #[arg(long)]
        no_hooks: bool,
        /// Не трогать CLAUDE.md / рекомендацию AGENTS.md.
        #[arg(long)]
        no_agents_md: bool,
        /// Добавить PostToolUse-гейт на каждую правку (только claude;
        /// дорого на репозиториях с command_succeeds-правилами).
        #[arg(long)]
        strict_hooks: bool,
        /// Писать в пользовательский конфиг хоста (~/.codex/config.toml,
        /// ~/.kimi-code/mcp.json) с мерджем и бэкапом вместо печати сниппета.
        #[arg(long)]
        apply_global: bool,
        /// Только показать план, ничего не записывать.
        #[arg(long)]
        dry_run: bool,
        /// Адрес релизов для джобы CI (только `connect ci`): подставляется в
        /// шаблон вместо заглушки `<org>/<repo>`. Без него джоба остаётся
        /// черновиком, и об этом сказано в «Следующих шагах» и в `doctor`.
        #[arg(long, value_name = "URL")]
        releases_url: Option<String>,
    },
}

/// Подкоманды `arch-be redteam` (ADR-051).
#[derive(Subcommand)]
enum RedteamCmd {
    /// Смысловая строка: что судья увидел в сохранённых клонах. В долю
    /// обнаружения не входит.
    SemanticScore {
        /// Каталог, переданный `redteam --keep-semantic`.
        dir: PathBuf,
        /// Формат вывода: text (дефолт) | json.
        #[arg(long, default_value = "text", value_name = "FORMAT")]
        format: String,
    },
}

pub(crate) async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let cfg = Arc::new(Config::load(cli.config.as_deref()).context("загрузка конфигурации")?);

    match cli.cmd {
        #[cfg(feature = "harness")]
        None | Some(Cmd::Tui) => arch_harness::tui::run(cfg).await?,
        // Core-сборка: TUI и агентного цикла нет — вместо запуска пустого
        // интерфейса печатаем краткую карту возможностей слим-сборки.
        #[cfg(not(feature = "harness"))]
        None => print_core_notice(),
        Some(Cmd::Init) => cmd_init(&cfg)?,
        #[cfg(feature = "harness")]
        Some(Cmd::Run {
            prompt,
            model,
            no_stream,
            quiet,
            timeout,
            max_turns,
            think,
        }) => {
            cmd_run(
                &cfg,
                prompt,
                model,
                !no_stream && !quiet,
                think,
                RunOptions {
                    timeout_secs: timeout,
                    max_turns,
                },
            )
            .await?;
        }
        Some(Cmd::Models) => {
            let registry = LlmRegistry::from_config(&cfg)?;
            println!("Модели (по умолчанию: {}):", registry.default_name());
            for name in registry.names() {
                let p = registry.get(&name)?;
                println!("  {name:<20} {} ({})", p.model(), p.name());
            }
        }
        #[cfg(feature = "harness")]
        Some(Cmd::Prompts { name }) => cmd_prompts(&cfg, name)?,
        Some(Cmd::Memory { cmd }) => cmd_memory(&cfg, cmd)?,
        Some(Cmd::Mermaid { file }) => {
            // Каталог — понятная подсказка со списком *.mmd, а не «os error 21».
            let input = if file != "-" && std::path::Path::new(&file).is_dir() {
                arch_harness::mermaid::read_diagram_source(std::path::Path::new(&file))?
            } else {
                read_file_or_stdin(&file)?
            };
            let art = arch_harness::mermaid::render(&input)?;
            println!("{art}");
        }
        Some(Cmd::Archify { cmd }) => cmd_archify(&cfg, cmd).await?,
        Some(Cmd::Rubric { cmd }) => cmd_rubric(&cfg, cmd).await?,
        Some(Cmd::Rules { cmd }) => cmd_rules(cmd)?,
        #[cfg(feature = "harness")]
        Some(Cmd::Bench { cmd }) => cmd_bench(&cfg, cmd).await?,
        Some(Cmd::Kb { query, limit }) => {
            let hits = arch_harness::kb::search(
                &cfg.knowledge.dirs,
                &cfg.knowledge.extensions,
                &query,
                limit,
            )
            .await?;
            for hit in &hits {
                println!(
                    "── {}:{} (score {:.1})",
                    hit.path.display(),
                    hit.line,
                    hit.score
                );
                println!("{}", hit.snippet);
            }
            if hits.is_empty() {
                println!("Ничего не найдено.");
            }
        }
        #[cfg(feature = "harness")]
        Some(Cmd::Web { cmd }) => cmd_web(&cfg, cmd).await?,
        Some(Cmd::Mcp { cmd }) => cmd_mcp(&cfg, cmd).await?,
        #[cfg(feature = "harness")]
        Some(Cmd::Handoff {
            harness,
            repo,
            task,
            spec,
            rollback,
            route,
            refresh_constraints,
        }) => {
            if !cfg.harnesses.contains_key(&harness) {
                anyhow::bail!(
                    "неизвестный харнесс '{harness}'. Известные: {:?}",
                    arch_harness::harness::known()
                );
            }
            let route: arch_harness::control::Route =
                route.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let packet = arch_harness::handoff::generate_handoff_opts(
                &repo,
                &task,
                &spec,
                &cfg,
                rollback.as_deref(),
                route,
                arch_harness::handoff::HandoffOptions {
                    refresh_constraints,
                },
            )?;
            println!("Handoff-пакет: {}", packet.dir.display());
            for f in &packet.files {
                println!("  {}", f.display());
            }
            println!("epic-context ≈ {} токенов", packet.epic_context_tokens);
            match &packet.baseline {
                Some(h) => println!(
                    "git: {}baseline {h} (якорь отката)",
                    if packet.git_initialized {
                        "инициализирован, "
                    } else {
                        ""
                    }
                ),
                None => println!("⚠ git недоступен — якоря отката нет"),
            }
            for w in &packet.warnings {
                println!("⚠ {w}");
            }
            println!(
                "маршрут {route} → рекомендованный timeout_secs={}",
                packet.recommended_timeout_secs
            );
            if packet.git_dirty_tracked {
                println!(
                    "⚠ незакоммиченные изменения отслеживаемых файлов: откат на baseline их потеряет"
                );
            }
            // T-02: гейт читает пакетную копию реестра первой — что в неё
            // попало, видно в выводе, а не только в файле.
            match &packet.constraints_action {
                Some(action) => println!("реестр правил пакета: {action}"),
                None => println!(
                    "реестр правил пакета: существующий файл не тронут (--refresh-constraints перезапишет)"
                ),
            }
        }
        #[cfg(feature = "harness")]
        Some(Cmd::HarnessRun {
            harness,
            repo,
            task,
        }) => {
            use arch_harness::harness::Termination;
            let hcfg = cfg
                .harnesses
                .get(&harness)
                .with_context(|| format!("харнесс '{harness}' не настроен"))?;
            // [fleet] require_worktree: прогон изолируется в git worktree
            // (ветка arch/<run-id>), основное дерево не трогается; мерж —
            // только гейтом владельца (`arch fleet merge <run-id>`).
            let (repo, run_id) =
                match arch_harness::harness::enforce_run_worktree(&cfg, &repo, &harness).await? {
                    Some((dir, run_id)) => {
                        println!(
                            "⚑ [fleet] require_worktree: прогон изолирован в worktree \
                             arch/{run_id} ({}); основное дерево не изменяется",
                            dir.display()
                        );
                        println!(
                            "  интеграция — гейт владельца: arch fleet merge {run_id} \
                             [--owner-approve]; отклонение: arch worktree drop {run_id}"
                        );
                        (dir, Some(run_id))
                    }
                    None => (repo, None),
                };
            let task_text = match task {
                Some(t) => t,
                None => std::fs::read_to_string(repo.join(".arch-handoff/TASK.md"))
                    .context("нет --task и не найден .arch-handoff/TASK.md")?,
            };
            let mut hcfg_owned = hcfg.clone();
            if let Some(t) = arch_harness::handoff::recommended_timeout_secs(&repo) {
                // Пакет несёт рекомендацию по маршруту значимости (Fast/Standard/Critical).
                hcfg_owned.timeout_secs = t.clamp(600, 7200);
            }
            let run = arch_harness::harness::run_harness(&harness, &hcfg_owned, &repo, &task_text)
                .await?;
            if let Some(ac) = &run.auto_commit {
                println!(
                    "⚑ авто-коммит: исполнитель не зафиксировал результат — {} путей → {} «{}»",
                    ac.files, ac.hash, ac.message
                );
            }
            match &run.contract {
                arch_harness::harness::ContractParse::Valid(c) => {
                    println!(
                        "контракт: status={} assumptions={} open_questions={} conflicts={}",
                        c.status.as_str(),
                        c.assumptions.len(),
                        c.open_questions.len(),
                        c.conflicts.len()
                    );
                }
                arch_harness::harness::ContractParse::Invalid(r) => {
                    eprintln!("⚠ контракт найден, но невалиден по схеме: {r}");
                }
                arch_harness::harness::ContractParse::Missing => {
                    eprintln!("⚠ контракт результата (```json со status) в stdout не найден");
                }
            }
            if let Some(id) = &run_id {
                println!(
                    "прогон изолирован в worktree arch/{id}: мерж — arch fleet merge {id} \
                     --owner-approve, отклонение — arch worktree drop {id}"
                );
            }
            if run.termination != Termination::Completed {
                eprintln!(
                    "⚠ прогон ПРЕРВАН ({}{}); процессная группа завершена, \
                     репозиторий может быть в промежуточном состоянии — проверьте git status",
                    run.termination,
                    if run.termination == Termination::IdleTimeout {
                        format!(" {} с", hcfg.idle_timeout_secs)
                    } else {
                        format!(" {} с", hcfg.timeout_secs)
                    }
                );
            }
            println!(
                "── stdout (exit {:?}, {:.1}s) ──",
                run.exit_code, run.duration_secs
            );
            println!("{}", run.stdout);
            if !run.stderr.is_empty() {
                eprintln!("── stderr ──\n{}", run.stderr);
            }
            // Скриптовый гейт: status=blocked — код 2; непустые
            // conflicts_with_prior_decisions — код 3 (конфликт со spine
            // останавливает интеграцию по контракту). Полная схема кодов —
            // docs/harness_integrations.md.
            if let arch_harness::harness::ContractParse::Valid(c) = &run.contract {
                if c.status == arch_harness::harness::ContractStatus::Blocked {
                    std::process::exit(2);
                }
                if !c.conflicts.is_empty() {
                    std::process::exit(3);
                }
            }
        }
        #[cfg(feature = "harness")]
        Some(Cmd::Harnesses) => {
            println!("Известные кодовые харнессы:");
            for name in arch_harness::harness::known() {
                let status = match cfg.harnesses.get(name) {
                    Some(h) => format!("{} ({:?})", h.binary, h.prompt_mode),
                    None => "не настроен".into(),
                };
                let installed = which(cfg.harnesses.get(name).map_or(name, |h| h.binary.as_str()));
                println!("  {name:<14} {status:<40} {installed}");
            }
        }
        Some(Cmd::Control { cmd }) => cmd_control(&cfg, cmd)?,
        Some(Cmd::Gate {
            repo,
            route,
            base,
            constraints,
            format,
            verify_envelope,
            explain,
            no_exec,
        }) => {
            let repo = repo.unwrap_or_else(|| PathBuf::from("."));
            // Режим сверки конверта: пересчитывает входы на текущем дереве и
            // отвечает, относится ли вердикт к этому состоянию.
            if let Some(envelope) = verify_envelope {
                let limits = cfg
                    .significance
                    .limits()
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let drift = arch_harness::gate::verify_envelope(&repo, &envelope, limits)?;
                if drift.same {
                    println!(
                        "Вердикт относится к этому состоянию: {}",
                        envelope.display()
                    );
                } else {
                    println!(
                        "Состояние изменилось с момента вердикта ({}) :",
                        envelope.display()
                    );
                    for (name, was, now) in &drift.changed {
                        println!("  {name}: {was} → {now}");
                    }
                    for name in &drift.missing {
                        println!("  {name}: был в вердикте, сейчас отсутствует");
                    }
                    println!(
                        "Перепроверьте: arch-be gate --repo {} --format json \
                         > verdict.json",
                        repo.display()
                    );
                    std::process::exit(1);
                }
                return Ok(());
            }
            let route = match route.trim().to_ascii_lowercase().as_str() {
                "auto" => None,
                other => Some(
                    other
                        .parse::<arch_harness::control::Route>()
                        .map_err(|e: String| anyhow::anyhow!(e))?,
                ),
            };
            // `--format json` — конверт вердикта (П7); остальные форматы —
            // через общий рендер report_fmt.
            let json_envelope = format.trim().eq_ignore_ascii_case("json");
            let format = if json_envelope {
                arch_harness::report_fmt::ReportFormat::Text
            } else {
                arch_harness::report_fmt::ReportFormat::parse(&format)
                    .map_err(anyhow::Error::msg)?
            };
            // Пороги маршрутов — из конфига ([significance], ADR-034).
            let limits = cfg
                .significance
                .limits()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            // A3: политика исполнения command_succeeds — снимок CLI-края
            // (--no-exec / ARCH_NO_EXEC / allow-файл), библиотека получает
            // готовое решение (детерминированные дефолты не тронуты, AD-7).
            let gate_options = arch_harness::gate::GateOptions {
                exec: arch_harness::cmd_trust::ExecPolicy::cli(no_exec),
                ..arch_harness::gate::GateOptions::from_config(&cfg)
            };
            let report = arch_harness::gate::run_opts(
                &repo,
                route,
                base.as_deref(),
                constraints.as_deref(),
                limits,
                &arch_harness::gate::GateRequirements::from_config(&cfg.gate),
                &gate_options,
            )?;
            // Паспорт вердикта (W1): строится ДО печати, но вердикт не
            // меняет — страница описывает тот же прогон, а не второй.
            let passport = explain.then(|| arch_harness::passport::Passport::build(&report, &repo));
            if json_envelope {
                let mut envelope = report.envelope_json();
                if let Some(passport) = &passport {
                    // Аддитивный ключ: контракт `gate-verdict/v1` не ломается.
                    envelope["passport"] = passport.to_json();
                }
                let text = serde_json::to_string_pretty(&envelope)
                    .unwrap_or_else(|_| envelope.to_string());
                println!("{text}");
            } else if let Some(passport) = &passport {
                // Одна страница вместо отчёта: паспорт — надмножество
                // (блок 1 несёт те же составляющие с теми же числами).
                print!("{}", passport.render());
            } else {
                match format {
                    arch_harness::report_fmt::ReportFormat::Text => {
                        print!("{}", arch_harness::gate::render(&report));
                    }
                    machine => {
                        // Машинные форматы — строго в stdout (артефакт CI);
                        // exit-код тот же, что у текста.
                        print!(
                            "{}",
                            arch_harness::report_fmt::render(
                                machine,
                                &arch_harness::report_fmt::FmtReport::from_gate(&report),
                            )
                        );
                    }
                }
            }
            let code = report.outcome.exit_code();
            if code != 0 {
                std::process::exit(code);
            }
        }
        Some(Cmd::Trust { dir, format }) => {
            // A3: прогон гейта внутри оценки наследует CLI-политику доверия
            // (ARCH_NO_EXEC / allow-файл; флага у команды нет — она
            // диагностическая, свой контур).
            let trust = arch_harness::trust::assess_with(
                &dir,
                &cfg,
                &arch_harness::cmd_trust::ExecPolicy::cli(false),
            )?;
            if format.trim().eq_ignore_ascii_case("json") {
                let out = arch_harness::trust::to_json(&trust);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string())
                );
            } else {
                print!("{}", arch_harness::trust::render(&trust));
            }
        }
        Some(Cmd::Bootstrap {
            name,
            dir,
            domain,
            status,
        }) => {
            // Каталог: явный `--dir`, иначе транслит имени, иначе текущий.
            let dir = dir
                .or_else(|| {
                    name.as_deref()
                        .map(|n| PathBuf::from(arch_harness::bootstrap::slugify(n)))
                })
                .unwrap_or_else(|| PathBuf::from("."));
            if status {
                let progress = arch_harness::bootstrap::status(&dir, &cfg)?;
                print!("{}", arch_harness::bootstrap::render(&progress));
                return Ok(());
            }
            let Some(name) = name else {
                anyhow::bail!(
                    "укажите имя кейса: arch-be bootstrap \"Зарплатные выплаты\" \
                     [--dir кейсы/salary] либо --status для существующего кейса"
                );
            };
            let progress = arch_harness::bootstrap::create(&dir, &name, &domain, &cfg)?;
            println!(
                "Каркас создан: {} ({} файлов). Он красный — так и задумано.",
                dir.display(),
                arch_harness::bootstrap::created_files().len()
            );
            print!("{}", arch_harness::bootstrap::render(&progress));
        }
        Some(Cmd::Selftest {
            json,
            redteam,
            min_detection,
        }) => {
            let report = arch_harness::selftest::run();
            // Расширенный режим: к метаморфным инвариантам добавляется
            // мутационный прогон по кейсу (W2).
            let redteam_report = match redteam.as_deref() {
                Some(case) => Some(arch_harness::redteam::run(
                    case,
                    min_detection / 100.0,
                    !cfg.gate.required.critical.is_empty(),
                )?),
                None => None,
            };
            if json {
                let invariants: Vec<serde_json::Value> = report
                    .invariants
                    .iter()
                    .map(|i| {
                        serde_json::json!({
                            "name": i.name,
                            "passed": i.passed,
                            "detail": i.detail,
                        })
                    })
                    .collect();
                let mut out = serde_json::json!({
                    "tool": "arch-be selftest",
                    "passed": report.passed()
                        && redteam_report.as_ref().is_none_or(
                            arch_harness::redteam::RedteamReport::passed
                        ),
                    "invariants": invariants,
                });
                if let Some(rt) = &redteam_report {
                    out["redteam"] = rt.to_json();
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string())
                );
            } else {
                print!("{}", report.render());
                if let Some(rt) = &redteam_report {
                    println!();
                    print!("{}", rt.render());
                }
            }
            let redteam_ok = redteam_report
                .as_ref()
                .is_none_or(arch_harness::redteam::RedteamReport::passed);
            if !report.passed() || !redteam_ok {
                std::process::exit(1);
            }
        }
        Some(Cmd::Redteam {
            case,
            cmd,
            format,
            min_detection,
            no_decision_quality,
            save,
            keep_semantic,
        }) => {
            // Подкоманда `semantic-score` читает уже сохранённые клоны: сам
            // прогон кейса не нужен и кейс не обязателен.
            if let Some(RedteamCmd::SemanticScore { dir, format }) = cmd {
                let score = arch_harness::redteam::semantic_score(&dir, &cfg.paths.rubrics_dir())?;
                if format.trim().eq_ignore_ascii_case("json") {
                    let cases: Vec<serde_json::Value> = score
                        .cases
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "mutant": c.mutant,
                                "rubric": c.rubric,
                                "subject": c.subject,
                                "verdict": c.verdict.label(),
                                "judge": c.judge,
                                "judge_is_author": c.judge_is_author,
                            })
                        })
                        .collect();
                    let out = serde_json::json!({
                        "schema": "arch-be/semantic-score/v1",
                        "caught": score.caught(),
                        "total": score.cases.len(),
                        "cases": cases,
                        "note": "смысловой слой не входит в долю обнаружения red-team (ADR-051)",
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string())
                    );
                } else {
                    println!("{}", score.render());
                }
                return Ok(());
            }
            let Some(case) = case else {
                anyhow::bail!(
                    "укажите кейс: `arch-be redteam <кейс>` или подкоманду \
                     `arch-be redteam semantic-score <каталог>`"
                );
            };
            let report = arch_harness::redteam::run_with_options(
                &case,
                &arch_harness::redteam::RedteamOptions {
                    min_detection,
                    decision_quality: !no_decision_quality,
                    keep_semantic: keep_semantic.clone(),
                },
            )?;
            if save {
                // Сохраняем в ИСХОДНЫЙ кейс: прогон шёл в копии.
                let path = arch_harness::redteam::save_summary(&case, &report)?;
                eprintln!("Итог измерения сохранён: {}", path.display());
            }
            match format.trim().to_ascii_lowercase().as_str() {
                "json" => {
                    let out = report.to_json();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string())
                    );
                }
                "markdown" | "md" => print!("{}", arch_harness::redteam::render_markdown(&report)),
                _ => print!("{}", report.render()),
            }
            if !report.passed() {
                std::process::exit(1);
            }
        }
        Some(Cmd::Adr { cmd }) => cmd_adr(cmd)?,
        Some(Cmd::Review {
            dir,
            base,
            constraints,
            json,
            no_exec,
        }) => {
            // Пороги маршрутов — из конфига ([significance], ADR-034).
            let limits = cfg
                .significance
                .limits()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            // A3: политика исполнения command_succeeds — снимок CLI-края,
            // как у `gate` (--no-exec / ARCH_NO_EXEC / allow-файл).
            let report = arch_harness::review::architect_review_opts(
                &dir,
                base.as_deref(),
                constraints.as_deref(),
                limits,
                &arch_harness::cmd_trust::ExecPolicy::cli(no_exec),
            )?;
            if json {
                let verdict = arch_harness::review::review_json(&report);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string())
                );
            } else {
                print!("{}", arch_harness::review::render_review(&report));
            }
            let code = report.gate.outcome.exit_code();
            if code != 0 {
                std::process::exit(code);
            }
        }
        Some(Cmd::Publish { cmd }) => cmd_publish(cmd)?,
        Some(Cmd::ContractDiff {
            old,
            new,
            contract_format,
            format,
            model,
            json,
        }) => {
            let lang = match contract_format.trim() {
                "auto" => None,
                other => Some(
                    arch_harness::contract_diff::ContractFormat::from_name(other)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "неизвестный формат контракта '{other}' (допустимы: auto, openapi, proto, avro, jsonschema, ddl)"
                            )
                        })?,
                ),
            };
            let out_format = arch_harness::report_fmt::ReportFormat::parse(&format)
                .map_err(anyhow::Error::msg)?;
            let report =
                arch_harness::contract_diff::diff_report(&old, &new, lang, model.as_deref())?;
            if json {
                let verdict = arch_harness::contract_diff::report_json(&report);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string())
                );
            } else {
                match out_format {
                    arch_harness::report_fmt::ReportFormat::Text => {
                        print!("{}", arch_harness::contract_diff::render_report(&report));
                    }
                    machine => {
                        print!(
                            "{}",
                            arch_harness::report_fmt::render(
                                machine,
                                &arch_harness::report_fmt::FmtReport::from_contract_diff(
                                    &report.findings,
                                ),
                            )
                        );
                    }
                }
            }
            if report.has_breaking() {
                std::process::exit(1);
            }
        }
        Some(Cmd::Model { cmd }) => cmd_model(cmd)?,
        Some(Cmd::Trace { cmd }) => cmd_trace(&cfg, cmd)?,
        Some(Cmd::Nfr { cmd }) => cmd_nfr(cmd)?,
        Some(Cmd::Skills { cmd }) => cmd_skills(&cfg, cmd)?,
        Some(Cmd::Plugins { cmd }) => cmd_plugins(&cfg, cmd)?,
        Some(Cmd::Policy { check }) => cmd_policy(&cfg, check)?,
        Some(Cmd::Evidence { cmd }) => cmd_evidence(&cfg, cmd)?,
        Some(Cmd::Metrics { cost_report }) => {
            if cost_report {
                // Смета по реальным записям usage журналов (тарифы — из конфига).
                let report =
                    arch_harness::metrics::cost_report(&cfg.paths.sessions_dir, &cfg.models);
                print!("{}", arch_harness::metrics::render_cost_report(&report));
                return Ok(());
            }
            let mut m =
                arch_harness::metrics::collect(&cfg.paths.sessions_dir, &cfg.paths.reports_dir)?;
            // Денежная стоимость — только по тарифам моделей из конфига
            // (None — тарифы не заданы, выдуманного курса нет).
            m.total_cost =
                arch_harness::metrics::cost_report(&cfg.paths.sessions_dir, &cfg.models).total_cost;
            // Architecture drift по реестру AGENTS.md (repos.txt), если он ведётся.
            let registry = arch_harness::config::Config::home_dir().join("repos.txt");
            if registry.is_file() {
                if let Ok(report) = arch_harness::agentsmd::lint_registry(&registry) {
                    m.agentsmd_total = report.len();
                    m.agentsmd_stale = report
                        .iter()
                        .filter(|(_, issues)| {
                            issues
                                .iter()
                                .any(|i| i.rule.contains("stale") || i.severity == "error")
                        })
                        .count();
                }
            }
            println!("{}", m.to_markdown());
        }
        Some(Cmd::Digest {
            repo,
            week,
            days,
            json,
        }) => {
            let _ = week; // неделя — дефолтное окно; флаг принят для читаемости вызова
            let repo = repo.unwrap_or_else(|| PathBuf::from("."));
            let days = days.unwrap_or(arch_harness::digest::DEFAULT_WINDOW_DAYS);
            let report = arch_harness::digest::build(&repo, days)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("DigestReport сериализуется")
                );
            } else {
                print!("{}", arch_harness::digest::render_markdown(&report));
            }
        }
        Some(Cmd::Doctor { host, dir }) => {
            let checks = if let Some(raw) = host {
                let host = arch_harness::connect::Host::parse(&raw).map_err(anyhow::Error::msg)?;
                let dir = match dir {
                    Some(d) => d,
                    None => std::env::current_dir().context("cwd")?,
                };
                let checks =
                    arch_harness::doctor::run_host_checks(host, &dir, dirs::home_dir().as_deref());
                print!("{}", arch_harness::doctor::render_host(host, &checks));
                checks
            } else {
                let checks = arch_harness::doctor::run_checks(&cfg);
                print!("{}", arch_harness::doctor::render(&checks));
                checks
            };
            if arch_harness::doctor::exit_code(&checks) != 0 {
                std::process::exit(1);
            }
        }
        Some(Cmd::Export {
            format,
            session,
            out,
        }) => {
            let Some(fmt) = arch_harness::export::ExportFormat::parse(&format) else {
                return Err(anyhow::anyhow!(
                    "неизвестный формат «{format}» (ожидалось word|excel)"
                ));
            };
            let n = arch_harness::export::export_journal(&session, fmt, &out)?;
            println!("экспортировано {n} строк → {}", out.display());
        }
        Some(Cmd::Delta { cmd }) => cmd_delta(cmd)?,
        Some(Cmd::Openspec { cmd }) => cmd_openspec(cmd)?,
        Some(Cmd::AgentsMd { cmd }) => cmd_agents_md(&cfg, cmd)?,
        #[cfg(feature = "harness")]
        Some(Cmd::Cron { cmd }) => cmd_cron(&cfg, cmd).await?,
        #[cfg(feature = "harness")]
        Some(Cmd::Eval { cmd }) => cmd_eval(&cfg, cmd).await?,
        #[cfg(feature = "harness")]
        Some(Cmd::Worktree { cmd }) => cmd_worktree(&cfg, cmd).await?,
        Some(Cmd::Fleet { cmd }) => cmd_fleet(&cfg, cmd).await?,
        Some(Cmd::Survey { repo, out }) => cmd_survey(&repo, out.as_deref())?,
        Some(Cmd::Archunit { cmd }) => cmd_archunit(cmd).await?,
        Some(Cmd::Connect {
            host,
            provider,
            dir,
            rw,
            no_skills,
            no_hooks,
            no_agents_md,
            strict_hooks,
            apply_global,
            dry_run,
            releases_url,
        }) => {
            let dir = match dir {
                Some(d) => d,
                None => std::env::current_dir().context("cwd")?,
            };
            // Режим записи в подключении: без флага — read-only, `--rw` —
            // полный, `--rw=reports` — только отчёты рубрики (J7).
            let rw_mode = arch_harness::mcp_server::ServeMode::parse_rw(rw.as_deref())
                .map_err(anyhow::Error::msg)?;
            let rw_full = matches!(rw_mode, arch_harness::mcp_server::ServeMode::ReadWrite);
            let rw_reports = matches!(rw_mode, arch_harness::mcp_server::ServeMode::Reports);
            let special = host.trim().to_ascii_lowercase();
            if special == "ci" || special == "git-hooks" || special == "githooks" {
                // Гейты, не зависящие от хоста (волна 2, п.8): флаги агентных
                // хостов здесь неприменимы — отклоняем явно, чтобы не
                // молча игнорировать.
                if releases_url.is_some() && special != "ci" {
                    return Err(anyhow::anyhow!(
                        "--releases-url применим только к `connect ci`"
                    ));
                }
                if rw.is_some()
                    || no_skills
                    || no_hooks
                    || no_agents_md
                    || strict_hooks
                    || apply_global
                {
                    return Err(anyhow::anyhow!(
                        "флаги --rw/--no-skills/--no-hooks/--no-agents-md/--strict-hooks/--apply-global применимы только к хостам агентов, не к `connect {special}`"
                    ));
                }
                if special == "ci" {
                    let raw = provider.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("connect ci: укажите --provider gitlab|github|jenkins")
                    })?;
                    let provider = arch_harness::connect::CiProvider::parse(raw)
                        .map_err(anyhow::Error::msg)?;
                    let report = arch_harness::connect::connect_ci(
                        provider,
                        &dir,
                        dry_run,
                        releases_url.as_deref(),
                    )?;
                    print!(
                        "{}",
                        arch_harness::connect::render_plan(
                            &format!("CI-джоба Spine ({}) — {}", provider.name(), dir.display()),
                            &report,
                        )
                    );
                } else {
                    if provider.is_some() {
                        return Err(anyhow::anyhow!("--provider применим только к `connect ci`"));
                    }
                    let report = arch_harness::connect::connect_git_hooks(&dir, dry_run)?;
                    print!(
                        "{}",
                        arch_harness::connect::render_plan(
                            &format!("Git-хуки Spine — {}", dir.display()),
                            &report,
                        )
                    );
                }
                return Ok(());
            }
            if provider.is_some() {
                return Err(anyhow::anyhow!("--provider применим только к `connect ci`"));
            }
            let host = arch_harness::connect::Host::parse(&host).map_err(anyhow::Error::msg)?;
            let opts = arch_harness::connect::ConnectOptions {
                host,
                dir,
                rw: rw_full,
                rw_reports,
                skills: !no_skills,
                hooks: !no_hooks,
                agents_md: !no_agents_md,
                strict_hooks,
                apply_global,
                dry_run,
                home: dirs::home_dir(),
                plugins_dirs: cfg.plugins.dirs.clone(),
            };
            let report = arch_harness::connect::connect(&opts)?;
            print!("{}", arch_harness::connect::render_report(&opts, &report));
        }
    }
    Ok(())
}

/// Единый резолвер реестра ограничений для CLI-арм (E2): явный путь →
/// пакетная копия (`.arch-handoff/CONSTRAINTS.yaml`) → корневая
/// (`CONSTRAINTS.yaml`); ни одной копии — канонический дефолт, чтобы
/// ошибка «файл не читается» ссылалась на пакетный путь.
pub(crate) fn resolve_constraints_cli(repo: &Path, explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| {
        arch_harness::control::resolve_constraints_path(repo, None)
            .unwrap_or_else(|| repo.join(arch_harness::control::HANDOFF_CONSTRAINTS_PATH))
    })
}

/// Заставка core-сборки при запуске без аргументов: TUI и агентный цикл —
/// принадлежность фичи `harness`; здесь — карта возможностей слим-сборки
/// и указание, как собрать полную.
#[cfg(not(feature = "harness"))]
fn print_core_notice() {
    println!(
        "arch-be {version} — core-сборка (без TUI, агентного цикла и сетевых LLM-провайдеров).\n\
         \n\
         Это «орган» внешнего CLI-агента: архитектурный контроль через MCP и CLI.\n\
         \x20 MCP-сервер для хоста:  arch-be mcp serve [--rw]\n\
         \x20 Подключение хоста:     arch-be connect <claude|qwen|codex|kimi|omp|generic>\n\
         \x20 Диагностика окружения: arch-be doctor\n\
         \x20 Все команды сборки:    arch-be --help\n\
         \n\
         Полная сборка (TUI, агент, сетевые LLM): cargo build --release\n\
         (фича `harness` включена по умолчанию; слим-сборка: \
         --no-default-features --features core)",
        version = env!("CARGO_PKG_VERSION")
    );
}

/// Читает файл или stdin (`-`).
fn read_file_or_stdin(file: &str) -> Result<String> {
    if file == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("чтение stdin")?;
        Ok(buf)
    } else {
        std::fs::read_to_string(file).with_context(|| format!("чтение {file}"))
    }
}

/// Резолвит имя ассета: точный путь, либо `<dir>/<name>`, либо `<dir>/<name>.<ext>`.
pub(crate) fn resolve_asset(dir: &std::path::Path, name: &str, ext: &str) -> PathBuf {
    let as_path = PathBuf::from(name);
    if as_path.is_file() {
        return as_path;
    }
    let in_dir = dir.join(name);
    if in_dir.is_file() {
        return in_dir;
    }
    dir.join(format!("{name}.{ext}"))
}

/// Есть ли бинарь в PATH (используется сводкой `arch-be harnesses`).
#[cfg(feature = "harness")]
fn which(binary: &str) -> String {
    std::process::Command::new("which")
        .arg(binary)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "MISSING".into(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    /// Рендерит long-help подкоманды по пути (например, `["mcp", "serve"]`).
    fn long_help(path: &[&str]) -> String {
        let mut cmd = Cli::command();
        for (i, name) in path.iter().enumerate() {
            let sub = cmd
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("нет подкоманды '{name}'"));
            if i + 1 == path.len() {
                return sub.clone().render_long_help().to_string();
            }
            cmd = sub.clone();
        }
        unreachable!("путь подкоманды пуст")
    }

    /// A4: справка `arch-be mcp serve` перечисляет ВСЕ инструменты реестра
    /// MCP-сервера (ручные + read-only мост + rw-список `--rw`) — тест
    /// падает при расхождении help-текста с константами `mcp_server`,
    /// справка больше не протухает.
    #[test]
    fn mcp_serve_help_lists_all_registry_tools() {
        let help = long_help(&["mcp", "serve"]);
        for name in arch_harness::mcp_server::MANUAL_TOOLS
            .iter()
            .chain(arch_harness::mcp_server::BRIDGE_READ_ONLY)
        {
            assert!(
                help.contains(name),
                "справка `mcp serve` не перечисляет read-only инструмент '{name}'"
            );
        }
        for rw in arch_harness::mcp_server::BRIDGE_READ_WRITE {
            assert!(
                help.contains(rw),
                "справка `mcp serve --rw` не перечисляет rw-инструмент '{rw}'"
            );
        }
    }

    /// A4: справка `arch-be connect --rw` перечисляет полный rw-список
    /// реестра (включая `evidence_pack` и `delta_propose`).
    #[test]
    fn connect_help_lists_full_rw_bridge() {
        let help = long_help(&["connect"]);
        for rw in arch_harness::mcp_server::BRIDGE_READ_WRITE {
            assert!(
                help.contains(rw),
                "справка `connect --rw` не перечисляет rw-инструмент '{rw}'"
            );
        }
    }
}
