//! Тонкая точка входа: парсинг аргументов → вызов lib → код возврата.

use std::io::Read as _;
#[cfg(feature = "harness")]
use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "harness")]
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[cfg(feature = "harness")]
use arch_harness::agent::AgentSession;
use arch_harness::config::Config;
use arch_harness::llm::LlmRegistry;
#[cfg(feature = "harness")]
use arch_harness::tool::ToolContext;

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
        #[arg(long)]
        rw: bool,
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

/// Подкоманды `arch-be archunit` (ADR-039).
#[derive(Subcommand)]
enum ArchunitCmd {
    /// Сгенерировать артефакты для JVM-репо: `ArchFitnessTest.java` (`JUnit` 5 +
    /// `ArchUnit`, встраивается в репо команды) и `archunit-rules.json` (спек).
    Gen {
        /// JVM-репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Каталог типизированной модели (для `context_boundary`; дефолт
        /// <repo>/model).
        #[arg(long)]
        model_dir: Option<PathBuf>,
        /// Каталог вывода (по умолчанию <repo>/archunit-fitness).
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Базовый пакет для `@AnalyzeClasses` (без него — выводится из
        /// якорных пакетов спека, иначе сканируется всё `..`).
        #[arg(long)]
        base_package: Option<String>,
    },
    /// Standalone-гейт: исполнить java-правила `CONSTRAINTS.yaml` настоящим
    /// `ArchUnit` на скомпилированных классах (без правок JVM-репо).
    Check {
        /// JVM-репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Каталог типизированной модели (для `context_boundary`; дефолт
        /// <repo>/model).
        #[arg(long)]
        model_dir: Option<PathBuf>,
        /// Каталог скомпилированных классов (без него — авто-детект
        /// target/classes, build/classes/java/main, out/production, classes).
        #[arg(long)]
        classes: Option<PathBuf>,
        /// Каталог с jar'ами `ArchUnit` (без него — $`ARCHUNIT_HOME`, затем
        /// ~/.arch-harness/archunit/lib).
        #[arg(long)]
        jar_dir: Option<PathBuf>,
        /// Таймаут гейта, секунды (дефолт 300).
        #[arg(long)]
        timeout_secs: Option<u64>,
        /// Машиночитаемый вывод: JSON-отчёт.
        #[arg(long)]
        json: bool,
    },
    /// Скачать пиннутые jar'ы `ArchUnit` (archunit + slf4j) с Maven Central в
    /// кэш с проверкой SHA-256. Только сборка `harness` (сетевой стек).
    #[cfg(feature = "harness")]
    Fetch {
        /// Каталог назначения (по умолчанию ~/.arch-harness/archunit/lib).
        #[arg(long)]
        jar_dir: Option<PathBuf>,
    },
}

/// Подкоманды `arch-be memory`.
#[derive(Subcommand)]
enum MemoryCmd {
    /// Дописать заметку в конец файла памяти.
    Add {
        /// Текст заметки.
        text: String,
    },
}

/// Подкоманды `arch-be archify`.
#[derive(Subcommand)]
enum ArchifyCmd {
    /// Проверка окружения Archify (node, CLI, рендеры пяти типов).
    Doctor,
    /// Рекомендация типа диаграммы и сценария под запрос.
    Guide {
        /// Вопрос/сценарий на естественном языке.
        query: String,
    },
    /// Валидация IR: 9 artifact checks + composition-профиль.
    Validate {
        /// Тип диаграммы: architecture|workflow|sequence|dataflow|lifecycle.
        r#type: String,
        /// Путь к JSON IR.
        path: PathBuf,
        /// Composition-профиль приёмки: standard|showcase.
        #[arg(long, default_value = "showcase")]
        quality: String,
        /// Машиночитаемый вывод: сырой JSON-receipt Archify CLI (SDK-контракт v1).
        #[arg(long)]
        json: bool,
    },
    /// Финальная приёмка: атомарная доставка HTML + SHA-256 receipt.
    Deliver {
        /// Тип диаграммы: architecture|workflow|sequence|dataflow|lifecycle.
        r#type: String,
        /// Путь к JSON IR.
        path: PathBuf,
        /// Путь к выходному HTML.
        output: PathBuf,
        /// Composition-профиль приёмки: standard|showcase.
        #[arg(long, default_value = "showcase")]
        quality: String,
        /// Машиночитаемый вывод: сырой JSON-receipt Archify CLI (SDK-контракт v1).
        #[arg(long)]
        json: bool,
    },
    /// Дельта двух architecture-снапшотов (Before/Delta/After + receipt).
    Compare {
        /// Путь к базовому architecture JSON IR.
        base: PathBuf,
        /// Путь к целевому architecture JSON IR.
        head: PathBuf,
        /// Путь к выходному delta HTML.
        output: PathBuf,
        /// Composition-профиль приёмки: standard|showcase.
        #[arg(long, default_value = "showcase")]
        quality: String,
        /// Машиночитаемый вывод: сырой JSON-receipt Archify CLI (SDK-контракт v1).
        #[arg(long)]
        json: bool,
    },
}

/// Подкоманды `arch-be fleet`.
#[derive(Subcommand)]
enum FleetCmd {
    /// SSOT-аудит флота: точные дубли документации и дрейф копий спайна.
    /// Дрейф хотя бы одного файла (разное содержимое у владельцев одного
    /// пути; канон — majority-версия) — exit code 1 (гейт для CI).
    Audit {
        /// Каталоги-worktree (каждый с копией архитектурных файлов).
        paths: Vec<PathBuf>,
        /// Репозиторий: worktree перечисляются из `git worktree list`
        /// (добавляются к позиционным путям).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Сузить сканирование glob'ом (повторяемый), напр. --include 'model/**'.
        /// По умолчанию — **/*.md|yaml|yml|json без .git/target/node_modules/.arch-handoff.
        #[arg(long)]
        include: Vec<String>,
        /// Формат вывода: text (таблица + топ расхождений) или json.
        #[arg(long, default_value = "text")]
        format: String,
        /// Exit 1, если доля точных дублей выше порога (проценты, напр. 50).
        #[arg(long)]
        fail_on_dupes: Option<f64>,
    },
    /// Гейт мерджа результата прогона флота (worktree `arch/<run-id>`) в
    /// основную ветку — «агент не имеет пути в main», интеграцию подтверждает
    /// владелец. Без --owner-approve печатает сводку прогона (diff stat,
    /// коммиты ветки, статус контракта из лога-evidence) и ОТКАЗЫВАЕТ мержить
    /// (exit 1). Режим гейта — [fleet] `merge_gate` ("owner" по умолчанию,
    /// "none" — без гейта). Только сборка `harness` (worktree-фабрика).
    #[cfg(feature = "harness")]
    Merge {
        /// Run-id прогона (имя worktree без префикса arch/, напр.
        /// claude-code-20260825103000 — его сообщает `harness_run` при
        /// [fleet] `require_worktree` = true).
        run_id: String,
        /// Явное подтверждение владельца: выполнить merge в основную ветку.
        #[arg(long)]
        owner_approve: bool,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

/// Подкоманды `arch-be worktree` (только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
enum WorktreeCmd {
    /// Создать изолированный worktree (ветка arch/<name>).
    New {
        /// Имя (kebab-case [a-z0-9-]).
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Базовая ветка/коммит (по умолчанию HEAD).
        #[arg(long)]
        base: Option<String>,
    },
    /// Список worktree фабрики.
    List {
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Diff ветки worktree против HEAD (review).
    Diff {
        /// Имя worktree.
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Принять: merge в текущую ветку + уборка worktree.
    Accept {
        /// Имя worktree.
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Удалить worktree без merge (только чистое).
    Drop {
        /// Имя worktree.
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// Список всех скиллов библиотеки.
    List,
    /// Поиск по скиллам.
    Search {
        /// Запрос.
        query: String,
        /// Максимум результатов.
        #[arg(long, default_value_t = 8)]
        limit: usize,
    },
    /// Показать полный текст скилла.
    Show {
        /// Точное имя скилла.
        name: String,
    },
}

#[derive(Subcommand)]
enum PluginsCmd {
    /// Список плагинов.
    List,
    /// Подробности плагина (манифест, скиллы, MCP-серверы).
    Show {
        /// Имя плагина.
        name: String,
    },
}

#[derive(Subcommand)]
enum RubricCmd {
    /// Список якорных рубрик.
    List,
    /// Оценить файл по рубрике (LLM-судья).
    Run {
        /// Рубрика (имя файла в assets/rubrics или путь).
        rubric: String,
        /// Целевой документ (md/txt); для смысловой рубрики не нужен — там
        /// задаётся `--pack`/`--subject`.
        target: Option<PathBuf>,
        /// Вид досье смысловой рубрики (ADR-051): `adr_vs_spine` |
        /// `entity_links` | `nfr_mechanism` | `code_vs_spine`.
        #[arg(long)]
        pack: Option<String>,
        /// Субъект досье: путь к ADR или файлу кода либо идентификатор
        /// сущности модели — вместе с `--pack`.
        #[arg(long)]
        subject: Option<String>,
        /// Корень репозитория для сборки досье (по умолчанию — текущий каталог).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Модель-судья.
        #[arg(long)]
        model: Option<String>,
        /// Сначала сгенерировать динамическую рубрику под предмет.
        #[arg(long)]
        dynamic_subject: Option<String>,
        /// Модель-автор документа: `judge == author` — судья судил свою же
        /// работу (метка `judge_is_author` в составляющей `decision_quality`).
        #[arg(long)]
        author_model: Option<String>,
    },
    /// Собрать досье судьи (вход смысловой рубрики) и напечатать его с хэшем.
    Pack {
        /// Вид досье: `adr_vs_spine` | `entity_links` | `nfr_mechanism` |
        /// `code_vs_spine`.
        kind: String,
        /// Субъект: путь к ADR/файлу кода либо идентификатор сущности модели.
        subject: String,
        /// Корень репозитория (по умолчанию — текущий каталог).
        #[arg(long)]
        root: Option<PathBuf>,
    },
}

/// Подкоманды `arch-be rules`.
#[derive(Subcommand)]
enum RulesCmd {
    /// Кандидатные fitness-правила кейса (то же, что `control rules-suggest`).
    Suggest {
        /// Корень кейса (каталог с `docs/`, `model/`, `.arch-handoff/`).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Шаблоны исполняемых правил: библиотека, применение, проверка зубов.
    Template {
        #[command(subcommand)]
        cmd: RulesTemplateCmd,
    },
}

/// Подкоманды `arch-be rules template`.
#[derive(Subcommand)]
enum RulesTemplateCmd {
    /// Список шаблонов библиотеки.
    List,
    /// Показать шаблон: свойства, файлы, команды, словарь подбора.
    Show {
        /// Id шаблона.
        id: String,
    },
    /// Положить файлы шаблона в кейс и напечатать фрагмент правила.
    Apply {
        /// Id шаблона.
        id: String,
        /// Инвариант спайна, к которому привязывается правило (`AD-3`).
        #[arg(long)]
        ad: String,
        /// Корень кейса.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Язык поставки: python | java | both.
        #[arg(long, default_value = "python")]
        lang: String,
        /// Показать, что было бы сделано, ничего не записывая.
        #[arg(long)]
        dry_run: bool,
    },
    /// Проверка зубов: тест обязан падать на нарушающей реализации.
    Verify {
        /// Проверить все шаблоны библиотеки во временных каталогах.
        #[arg(long)]
        all: bool,
        /// Проверить применённые шаблоны кейса (по `.arch-handoff/rule-templates.lock`).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// JUnit-консоль (`junit-platform-console-standalone.jar`) для java-половины
        /// без Maven.
        #[arg(long)]
        java_jar: Option<PathBuf>,
        /// Язык проверки: python | java | both.
        #[arg(long, default_value = "both")]
        lang: String,
        /// Требовать python3: без него проверка считается проваленной (для CI).
        #[arg(long)]
        require_python: bool,
    },
}

/// Подкоманды `arch-be bench` (только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
enum BenchCmd {
    /// Список бенчмарков.
    List,
    /// Прогнать бенчмарк.
    Run {
        /// Имя файла бенчмарка в assets/benchmarks (или путь).
        name: Option<String>,
        /// Испытуемая модель (для --golden — модель-судья).
        #[arg(long)]
        model: Option<String>,
        /// Прогон судьи по golden-set (assets/benchmarks/golden): метрика
        /// согласия с эталоном MAE; выше порога `judge.golden_max_mae` — exit 1.
        #[arg(long)]
        golden: bool,
        /// Прогон одной рубрики: годен только с `--golden` (смысловые рубрики
        /// калибруются поимённо, ADR-051).
        #[arg(long)]
        rubric: Option<String>,
        /// Дописать результат golden-прогона строкой JSON в evidence-журнал
        /// (M-2, история — `bench golden-history`).
        #[arg(long)]
        record: Option<PathBuf>,
    },
    /// История golden-прогонов из evidence-журнала (JSONL) — markdown-таблица.
    GoldenHistory {
        /// Путь к журналу, записанному `bench run --golden --record`.
        path: PathBuf,
    },
    /// Согласие golden-эталонов с оценками живых архитекторов (J-3,
    /// протокол — docs/judge-human-agreement.md).
    HumanAgreement {
        /// Каталог golden-set (`<имя>.md` + `<имя>.expected.yaml`).
        #[arg(long)]
        golden_dir: PathBuf,
        /// Каталог человеческих анкет (`<документ>.<участник>.expected.yaml`).
        #[arg(long)]
        humans: PathBuf,
    },
}

/// Подкоманды `arch-be web` (только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
enum WebCmd {
    /// Поиск в вебе.
    Search {
        /// Запрос.
        query: String,
        /// Ограничить кураторскими архитектурными сайтами.
        #[arg(long)]
        arch: bool,
    },
    /// Загрузить страницу текстом.
    Fetch {
        /// URL.
        url: String,
    },
    /// Кураторский список сайтов архитектора.
    Sites,
}

#[derive(Subcommand)]
enum McpCmd {
    /// Список серверов и их инструментов.
    List,
    /// Вызвать MCP-инструмент.
    Call {
        /// Составное имя `server__tool`.
        name: String,
        /// Аргументы JSON.
        #[arg(default_value = "{}")]
        args: String,
    },
    /// MCP-сервер (stdio JSON-RPC, NDJSON): архитектурный контроль кодовым
    /// агентам (Claude Code и др.), ADR-008. Read-only состав: 36 инструментов
    /// + 8 промптов-плейбуков spine-* (capability prompts). Ручные (16):
    ///   `spine_lint`, `fitness_check`, `significance_score`,
    ///   `significance_from_diff`, `trace_check`, `model_query`, `rubric_run`,
    ///   `rubric_prompt`, `rubric_verify`, `kb_search`, `skill_search`,
    ///   `skill_load`, `mermaid_render`, `rules_suggest`, `trust_report`,
    ///   `verdict_explain`. Мостовые read-only
    ///   (20): `adr_registry`, `agentsmd_lint`, `archify_validate`,
    ///   `architect_review`, `asyncapi_lint`, `change_impact`, `contract_diff`,
    ///   `delta_guard`, `evidence_verify`, `fleet_audit`, `landscape_report`,
    ///   `model_drift`, `model_graph`, `model_validate`, `nfr_check`,
    ///   `openapi_lint`, `openspec_coverage`, `plugin_list`, `rubric_list`,
    ///   `rule_template_list`, `rule_template_show`, `rules_report`.
    Serve {
        /// Открыть rw-контур моста (аддитивные записи в рабочий каталог
        /// клиента: `adr_new`, `agentsmd_generate`, `archify_compare`,
        ///   `archify_deliver`, `archify_show`, `delta_propose`, `evidence_pack`,
        ///   `handoff_create`, `reverse_survey`, `rule_template_apply`,
        ///   `skill_distill`). По умолчанию
        /// сервер строго read-only.
        #[arg(long)]
        rw: bool,
    },
}

#[derive(Subcommand)]
enum ControlCmd {
    /// Fitness-контроль репозитория по `CONSTRAINTS.yaml`.
    Check {
        /// Репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Машиночитаемый вывод: JSON-отчёт `FitnessReport` (SDK-контракт v1).
        #[arg(long)]
        json: bool,
        /// Baseline-файл долга (JSON), режим ratchet для brownfield
        /// (`docs/control.md`): находки из baseline — долг (гейт не ломают),
        /// ломают только НОВЫЕ нарушения и рост счётчика правила.
        #[arg(long, value_name = "PATH")]
        baseline: Option<PathBuf>,
        /// Перезаписать baseline текущим состоянием. Принимается только при
        /// неухудшении долга (ratchet); без `--baseline` путь по умолчанию —
        /// <repo>/.arch-handoff/baseline.json.
        #[arg(long)]
        baseline_update: bool,
        /// Проверять только файлы, изменённые против `GIT_REF` (`git diff
        /// --name-only GIT_REF` по рабочему дереву + untracked): файловые
        /// правила — на срезе, глобальные — SKIP с пометкой. Для быстрых
        /// прогонов (PostToolUse-хуки); полный прогон остаётся истиной гейта.
        #[arg(long, value_name = "GIT_REF")]
        changed_since: Option<String>,
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
        /// База git для сверки состава правил (П5, анти-ослабление): по
        /// умолчанию — merge-base с основной веткой, иначе HEAD. Сверка
        /// добавляет находку `rule_weakened`, если правило исчезло или
        /// ослаблено; недоступность базы честно печатается в сводке.
        #[arg(long, value_name = "GIT_REF")]
        base: Option<String>,
    },
    /// Линтер ARCHITECTURE-SPINE.md.
    Spine {
        /// Путь к spine-файлу.
        file: PathBuf,
    },
    /// Сенсоры спецификаций (required-sections, upstream-coverage).
    Sensors {
        /// Каталог спецификаций.
        dir: PathBuf,
    },
    /// Architecture Significance Score: `--trigger new_component=true ...`
    Score {
        /// Триггеры вида имя=true/false.
        #[arg(long)]
        trigger: Vec<String>,
        /// Anti-bypass floor (ADR-034): механически вывести триггеры из
        /// git-диффа и объединить с заявленными (fail-safe — детектор только
        /// добавляет). Без значения — рабочее дерево против HEAD; со
        /// значением — `git diff GIT_REF...HEAD` (готовый диапазон `A...B`
        /// принимается как есть).
        #[arg(long, num_args = 0..=1, default_missing_value = "HEAD", value_name = "GIT_REF")]
        from_diff: Option<String>,
    },
    /// Отчёт по реестру правил `CONSTRAINTS.yaml` (сводка, таблица карточек,
    /// находки: без owner/expiry, просроченные, `exclude_glob`, git-прокси
    /// стоимости сопровождения, суммарный `effort_hours`).
    RulesReport {
        /// Репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
    },
    /// Кандидатные fitness-правила из содержательных пробелов кейса
    /// (read-only эвристики, `src/rules_suggest.rs`): EARS-критерии приёмки,
    /// численные таймауты в контрактах, декомпозиция REQ→работы, RTO/RPO без
    /// ADR, аудит операторских действий. Печать — markdown-отчёт + готовые
    /// YAML-фрагменты для `CONSTRAINTS.yaml` (взятие правила и severity —
    /// решение архитектора).
    RulesSuggest {
        /// Корень кейса (каталог с docs/, model/, .arch-handoff/).
        path: PathBuf,
    },
    /// Отчёт вверх по корпоративному контуру (наследование `extends`,
    /// `docs/corp-spine.md`): покрытие корп-правил, исходы (pass/fail/warn),
    /// overrides со статусами, просроченные правила, расхождения пинов версий.
    Report {
        /// Репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Уровень: corp (только унаследованные правила) | all (все).
        #[arg(long, default_value = "corp")]
        level: String,
        /// Машиночитаемый вывод: JSON-отчёт `ControlReport` (SDK-контракт v1).
        #[arg(long)]
        json: bool,
    },
    /// Новый ADR.
    Adr {
        /// Заголовок решения.
        title: String,
        /// Каталог ADR (по умолчанию ./docs/adr).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Гейт контрольной точки (пока A4 — conformance evidence: репетиция
    /// отката handoff-пакета, см. docs/control.md).
    Gate {
        /// Идентификатор гейта (реализован только A4).
        gate: String,
        /// Репозиторий (с .arch-handoff/) или каталог handoff-пакета.
        packet: PathBuf,
        /// Перед оценкой гейта прогнать репетицию отката (обновляет
        /// .arch-handoff/REHEARSAL.json).
        #[arg(long)]
        rehearse: bool,
        /// Репетиция обязательна для маршрутов не ниже порога:
        /// fast|standard|critical|never (дефолт critical).
        #[arg(long, default_value = "critical")]
        require_rehearsal: String,
    },
    /// Регистр ложных срабатываний правил (FP, `docs/outcome-metrics.md` §2).
    Fp {
        #[command(subcommand)]
        cmd: FpCmd,
    },
}

/// Подкоманды `arch-be control fp` (регистр ложных срабатываний).
#[derive(Subcommand)]
enum FpCmd {
    /// Пометить срабатывание правила как ложное: append строки
    /// `| дата | правило | файл | примечание |` в `evidence/fp-register.md`
    /// проекта (файл создаётся с шапкой при отсутствии).
    Mark {
        /// Имя правила из CONSTRAINTS.yaml.
        rule: String,
        /// Файл срабатывания (обычно `путь:строка`).
        file: String,
        /// Примечание (причина/решение: поправить правило / записать
        /// отступление / принять).
        #[arg(long)]
        note: Option<String>,
        /// Репозиторий проекта (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

/// Подкоманды `arch-be adr` (реестр ADR, ADR-036).
#[derive(Subcommand)]
enum AdrCmd {
    /// Глобальный реестр ADR по набору проектов: сам ROOT + непосредственные
    /// подкаталоги; источники — docs/adr/*.md (проза) и model/ADR-*.md
    /// (типизированные сущности ADR-003).
    Registry {
        /// Корневой каталог набора проектов.
        root: PathBuf,
        /// Машиночитаемый вывод: единый JSON {entries, findings}.
        #[arg(long)]
        json: bool,
        /// Exit 1 при любой находке (расхождение прозы и типизированной
        /// записи `prose_model_divergence`, коллизия номеров внутри одного
        /// представления, дубль заголовка, пропуск даты/статуса) — гейт для
        /// CI; по умолчанию exit 0.
        #[arg(long)]
        strict: bool,
    },
}

/// Подкоманды `arch-be publish` (файловые адаптеры, ADR-033).
#[derive(Subcommand)]
enum PublishCmd {
    /// Markdown → Confluence storage format (XHTML) в stdout: заголовки,
    /// таблицы, код-блоки, списки, инлайн-разметка (подмножество).
    Confluence {
        /// Markdown-файл (spine, ADR, evidence-индекс).
        file: PathBuf,
    },
    /// JSON результата handoff → Jira-CSV импорта (Summary,Type,Description,Labels).
    Jira {
        /// Файл результата handoff (`status`/`assumptions`/`open_questions`/…).
        result: PathBuf,
        /// Ключ проекта Jira — метка `spine-<ключ>` для фильтрации.
        #[arg(long)]
        project: Option<String>,
    },
}

/// Подкоманды `arch-be model` (ADR-003).
#[derive(Subcommand)]
enum ModelCmd {
    /// Ссылочная целостность модели: битая ссылка/дубль ID/цикл `depends_on` —
    /// error (exit code 1, как у `control check`); ADR без CMP, NFR без
    /// способа проверки — warn.
    Validate {
        /// Каталог модели.
        dir: PathBuf,
    },
    /// Карточка сущности: шапка, связи, обратные ссылки, тело.
    Show {
        /// ID сущности (ADR-001, CMP-002, …).
        id: String,
        /// Каталог модели (по умолчанию ./model).
        #[arg(long, default_value = "model")]
        dir: PathBuf,
    },
    /// Граф связей модели.
    Graph {
        /// Каталог модели (по умолчанию ./model).
        #[arg(long, default_value = "model")]
        dir: PathBuf,
        /// Формат: text (список) или mermaid (flowchart, совместим с `arch-be mermaid`).
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Проекция: рендер ADR-файлов из модели в <кейс>/.arch-handoff/adr/
    /// (зеркально; устаревшие ADR-*.md удаляются).
    Project {
        /// Каталог модели.
        dir: PathBuf,
    },
    /// Экспорт модели в отраслевой формат (ADR-009, ADR-032): Structurizr
    /// DSL, `PlantUML`, drawio (SYS/CMP/INT + связи) или `ArchiMate` Open
    /// Exchange 3.2 (SYS/CMP/INT/CAP/REQ/NFR/AD + связи) — на stdout.
    Export {
        /// Каталог модели.
        dir: PathBuf,
        /// Формат: structurizr, plantuml, drawio или archimate.
        #[arg(long)]
        format: String,
    },
    /// Импорт внешнего реестра/описания в модель: Structurizr DSL
    /// (SYS/CMP/INT + связи) либо реестр систем (csv/xlsx/backstage →
    /// `SYS-*` + `OWNER-*`; по одному .md на сущность; существующие —
    /// skip, перезапись — только `--force`, и на месте их файлов).
    Import {
        /// Файл-источник (Structurizr DSL, CSV, xlsx, catalog-info.yaml).
        file: PathBuf,
        /// Формат: structurizr, csv, xlsx, backstage.
        #[arg(long)]
        format: String,
        /// Каталог модели-получателя (создаётся при отсутствии).
        #[arg(long = "out", visible_alias = "dir", default_value = "model")]
        dir: PathBuf,
        /// Перезаписывать существующие сущности (только csv/xlsx/backstage).
        #[arg(long)]
        force: bool,
        /// Только план: разбор и отчёт без записи файлов
        /// (только csv/xlsx/backstage).
        #[arg(long)]
        dry_run: bool,
    },
    /// Дрейф «модель ↔ код» (read-only): CMP с несуществующими `code_roots`
    /// — error (exit code 1); каталог с манифестом сборки без покрывающего
    /// CMP — warn; звено `INT → контракт` в семантике `trace check`
    /// (ADR-035: битый путь `contract` — error, поле не задано — warn).
    Drift {
        /// Корень кейса (каталог с model/ внутри).
        dir: PathBuf,
        /// JSON-вердикт `{passed, issues, summary}` вместо текста.
        #[arg(long)]
        json: bool,
    },
    /// Радиус взрыва изменения (бэклог волны 3, п.13): от сущности (`--id`)
    /// или файлов (`--paths` → CMP по `code_roots`, ADR-030) транзитивный
    /// обход графа связей модели → затронутые сущности по типам, правила
    /// `CONSTRAINTS.yaml` (C-NNN с владельцами), контракты INT, владельцы
    /// OWNER — «что я задену и с кем согласовывать». Отчёт, не гейт.
    Impact {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
        /// ID сущности-источника (CMP-001, INT-002, …).
        #[arg(long)]
        id: Option<String>,
        /// Файл изменения (повторяемый флаг). Источники — CMP, чьи
        /// `code_roots` покрывают путь; непокрытые пути — в отчёте как gap.
        #[arg(long)]
        paths: Vec<String>,
        /// Машиночитаемый вывод: JSON-отчёт.
        #[arg(long)]
        json: bool,
    },
    /// Ландшафт систем набора проектов (EA-3, ADR-036/ADR-037): агрегация
    /// `model/` самого ROOT и непосредственных подкаталогов в единый
    /// реестр систем SYS/INT с дедупликацией по имени (без глобальных ID),
    /// находки (id-divergence, status-conflict, dangling-ref,
    /// cross-project-link) и топ связности.
    Landscape {
        /// Корневой каталог набора проектов.
        root: PathBuf,
        /// Дополнительно вывести mermaid `graph TD` ландшафта.
        #[arg(long)]
        mermaid: bool,
        /// Карта алиасов (yaml/json «вариант имени → каноничное имя»):
        /// дедупликация учитывает алиасы.
        #[arg(long)]
        aliases: Option<PathBuf>,
        /// Дифф ландшафта против версии в git: ссылка (ветка/тег/sha) или
        /// дата YYYY-MM-DD (последний коммит не позже конца дня).
        #[arg(long)]
        diff_since: Option<String>,
    },
}

/// Подкоманды `arch-be trace` (ADR-006).
#[derive(Subcommand)]
enum TraceCmd {
    /// Позвенная трассируемость: REQ → NFR → AD/ADR → CMP → правило
    /// `CONSTRAINTS.yaml`; AD без правила и без `unverifiable` — error
    /// (exit code 1). Отчёт markdown, пригоден для evidence bundle.
    Check {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
        /// Формат вывода: text (дефолт — markdown-отчёт звеньев) | sarif |
        /// junit | gitlab-codequality | markdown (нормализованная таблица
        /// находок, `src/report_fmt.rs`; машинные — в stdout).
        #[arg(long, default_value = "text", value_name = "FORMAT")]
        format: String,
    },
}

/// Подкоманды `arch-be nfr` (ADR-007).
#[derive(Subcommand)]
enum NfrCmd {
    /// Latency-бюджет: сумма бюджетов hop'ов INT-* против цели p99 из NFR-*;
    /// hop без бюджета или превышение — error (exit code 1).
    Budget {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
    },
    /// Доступность: композиция последовательных/параллельных участков против
    /// SLA из NFR-* + цели RTO/RPO; ниже SLA — error (exit code 1).
    Availability {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
    },
    /// Пропускная способность: RPS-цель против ёмкости компонентов
    /// (instances × `rps_per_instance`); дефицит — error (exit code 1).
    Capacity {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
    },
    /// Стоимость: TCO (инстансы × тариф) и цена выхода (Σ `exit_cost`)
    /// по тарифным данным сущностей.
    Cost {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum EvidenceCmd {
    /// Собрать bundle (EVIDENCE.yaml) по каталогу изменения.
    Pack {
        /// Каталог изменения.
        dir: PathBuf,
        /// Маршрут: fast|standard|critical.
        #[arg(long, default_value = "standard")]
        route: String,
    },
    /// Проверить bundle: полнота + целостность хэшей.
    Verify {
        /// Каталог изменения.
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum DeltaCmd {
    /// Новая дельта (каркас changes/<name>/DELTA.md).
    New {
        /// Имя изменения (kebab-case).
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Список дельт (предложенные/архивные).
    List {
        /// Репозиторий.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Валидация структуры дельты.
    Validate {
        /// Имя дельты.
        name: String,
        /// Репозиторий.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Архивировать дельту после apply (вливание в живую истину).
    Archive {
        /// Имя дельты.
        name: String,
        /// Репозиторий.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Гейт прямых правок спайна: изменённые защищённые файлы обязаны
    /// упоминаться в активной дельте changes/<name>/DELTA.md, иначе exit 1.
    /// Новые untracked-файлы git-diff не видит — для CI используйте --base.
    Guard {
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// База diff (по умолчанию HEAD — staged+unstaged рабочего дерева;
        /// для CI — напр. origin/main...HEAD: трёхточечную форму разбирает
        /// сам git).
        #[arg(long)]
        base: Option<String>,
        /// Защищаемый путь/префикс (повторяемый). Если задан хотя бы один —
        /// заменяет дефолт: model/, ARCHITECTURE-SPINE.md, `CONSTRAINTS.yaml`.
        #[arg(long)]
        protect: Vec<String>,
    },
}

/// Подкоманды `arch-be openspec` (адаптер `OpenSpec`, MVP; `docs/openspec.md`).
#[derive(Subcommand)]
enum OpenspecCmd {
    /// Список требований `OpenSpec`: живые спеки (openspec/specs/) и дельты
    /// активных changes; стабильный id `openspec:<capability>#<hash8>`,
    /// текст, источник (файл:строка).
    Scan {
        /// Корень репозитория с разметкой `OpenSpec`.
        root: PathBuf,
        /// Машиночитаемый вывод: JSON-отчёт `ScanReport`.
        #[arg(long)]
        json: bool,
    },
    /// Отчёт покрытия требований правилами CONSTRAINTS (связь — поле
    /// `covers:` правила): SHALL всего / покрыто детектором / unverifiable
    /// с owner / без решения; непокрытые — поимённо. Exit code: 0, если нет
    /// --strict; с --strict — 1 при наличии требований «без решения»
    /// (ни детектора, ни unverifiable с назначенным owner).
    Coverage {
        /// Корень репозитория с разметкой `OpenSpec`.
        root: PathBuf,
        /// Файл ограничений (по умолчанию <root>/.arch-handoff/CONSTRAINTS.yaml,
        /// иначе <root>/CONSTRAINTS.yaml; нет файла — все «без решения»).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Машиночитаемый вывод: JSON-отчёт `CoverageReport`.
        #[arg(long)]
        json: bool,
        /// Строгий режим: exit 1 при требованиях «без решения» (гейт CI).
        #[arg(long)]
        strict: bool,
    },
    /// Генерация артефактов перехода `OpenSpec` → Spine: скелет
    /// CONSTRAINTS.from-openspec.yaml (все SHALL как заглушки
    /// `unverifiable: true` с пустым owner и проставленным `covers:`),
    /// SPINE.draft.md (кандидаты из design.md активных changes) и печать
    /// отчёта покрытия. Существующие файлы не затираются без --force.
    Init {
        /// Корень репозитория с разметкой `OpenSpec`.
        root: PathBuf,
        /// Каталог вывода (по умолчанию — сам ROOT).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Перезаписать существующие файлы (регенерация детерминирована).
        #[arg(long)]
        force: bool,
    },
    /// Гейт архивации change (точка CI перед `openspec archive`): exit 1,
    /// если у требований change нет решения (ни детектора, ни unverifiable
    /// с owner) или падает `control check`. Реализован только --archive
    /// (roadmap: --change, --expiry — `docs/openspec.md`).
    Gate {
        /// Режим гейта: архивация change.
        #[arg(long)]
        archive: bool,
        /// Корень репозитория с разметкой `OpenSpec`.
        root: PathBuf,
        /// Идентификатор change (каталог openspec/changes/<id>).
        change_id: String,
        /// Файл ограничений (умолчание — как у `coverage`).
        #[arg(long)]
        constraints: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum AgentsMdCmd {
    /// Сгенерировать или обновить AGENTS.md (рукописная зона сохраняется).
    Refresh {
        /// Репозиторий.
        repo: PathBuf,
    },
    /// Проверить AGENTS.md: свежесть (дрейф источников), ссылки, заглушки.
    Lint {
        /// Репозиторий.
        repo: PathBuf,
    },
    /// Прогнать линтер по реестру репозиториев (файл: путь на строку).
    LintAll {
        /// Файл реестра (по умолчанию ~/.arch-harness/repos.txt).
        #[arg(long)]
        registry: Option<PathBuf>,
    },
}

/// Подкоманды `arch-be cron` (только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
enum CronCmd {
    /// Список задач расписания.
    List,
    /// Запустить задачу по имени сейчас.
    Run {
        /// Имя задачи.
        name: String,
    },
    /// Проверить и запустить дюжные задачи (для системного cron).
    Tick,
}

/// Подкоманды `arch eval` (continuous evals, docs/evals.md; только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
enum EvalCmd {
    /// Прогнать eval-сьют: детерминированные проверки (офлайн) + опциональный
    /// LLM-судья. Pass-rate ниже гейта — exit code 1 (регрессионный гейт).
    Run {
        /// Каталог сьюта (YAML-задачи). Без флага — встроенный сьют
        /// agent-config, прогоняемый герметично (ассеты разворачиваются во
        /// временный каталог; живой конфиг не трогается).
        #[arg(long)]
        suite: Option<PathBuf>,
        /// Гейт pass-rate в процентах (дефолт 100): ниже — exit code 1.
        #[arg(long)]
        gate: Option<f64>,
        /// Включить слой LLM-судьи (prompt-задачи и рубрики; нужен API-ключ).
        #[arg(long)]
        judge: bool,
        /// Модель для слоя судьи (имя из [models]; иначе — default).
        #[arg(long)]
        model: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
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
            let report = arch_harness::gate::run_opts(
                &repo,
                route,
                base.as_deref(),
                constraints.as_deref(),
                limits,
                &arch_harness::gate::GateRequirements::from_config(&cfg.gate),
                &arch_harness::gate::GateOptions::from_config(&cfg),
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
            let trust = arch_harness::trust::assess(&dir, &cfg)?;
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
        }) => {
            // Пороги маршрутов — из конфига ([significance], ADR-034).
            let limits = cfg
                .significance
                .limits()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let report = arch_harness::review::architect_review(
                &dir,
                base.as_deref(),
                constraints.as_deref(),
                limits,
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
                if rw || no_skills || no_hooks || no_agents_md || strict_hooks || apply_global {
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
                rw,
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

/// `arch-be archunit …`: `ArchUnit`-мост (ADR-039).
// В core-сборке единственный async-участок (fetch по сети) вырезан фичей —
// async-обёртка остаётся для единого вида с полной сборкой.
#[cfg_attr(not(feature = "harness"), allow(clippy::unused_async))]
async fn cmd_archunit(cmd: ArchunitCmd) -> Result<()> {
    match cmd {
        ArchunitCmd::Gen {
            repo,
            constraints,
            model_dir,
            out_dir,
            base_package,
        } => {
            let c = resolve_constraints_cli(&repo, constraints);
            let rules = arch_harness::control::load_fitness_rules(&c)?;
            let refs: Vec<&arch_harness::control::FitnessRule> = rules.iter().collect();
            let mut spec =
                arch_harness::archunit::spec_from_constraints(&repo, &refs, model_dir.as_deref());
            if let Some(bp) = base_package {
                spec.base_package = Some(bp);
            }
            let out = out_dir.unwrap_or_else(|| repo.join("archunit-fitness"));
            std::fs::create_dir_all(&out)
                .with_context(|| format!("не создать каталог вывода {}", out.display()))?;
            let test_path = out.join("ArchFitnessTest.java");
            let json_path = out.join("archunit-rules.json");
            std::fs::write(&test_path, arch_harness::archunit::render_junit_test(&spec))
                .with_context(|| format!("не записать {}", test_path.display()))?;
            std::fs::write(&json_path, arch_harness::archunit::spec_to_json(&spec)?)
                .with_context(|| format!("не записать {}", json_path.display()))?;
            println!(
                "спек: {} правил ArchUnit, {} не смаплено (unsupported)",
                spec.rules.len(),
                spec.unsupported.len()
            );
            for u in &spec.unsupported {
                println!("  [warn] {}: {}", u.rule, u.reason);
            }
            println!("JUnit-тест: {}", test_path.display());
            println!("спек JSON:  {}", json_path.display());
        }
        ArchunitCmd::Check {
            repo,
            constraints,
            model_dir,
            classes,
            jar_dir,
            timeout_secs,
            json,
        } => {
            let c = resolve_constraints_cli(&repo, constraints);
            let rules = arch_harness::control::load_fitness_rules(&c)?;
            let refs: Vec<&arch_harness::control::FitnessRule> = rules.iter().collect();
            let spec =
                arch_harness::archunit::spec_from_constraints(&repo, &refs, model_dir.as_deref());
            for u in &spec.unsupported {
                eprintln!("[warn] unsupported: {}: {}", u.rule, u.reason);
            }
            let classes_dir = match classes {
                Some(dir) => dir,
                None => arch_harness::archunit::find_classes_dir(&repo).with_context(|| {
                    "archunit: скомпилированные классы не найдены (target/classes, \
                     build/classes/java/main, out/production, classes) — соберите проект \
                     (`mvn compile` / `javac -d classes ...`) или укажите --classes"
                })?,
            };
            let opts = arch_harness::archunit::GateOptions {
                classes_dir,
                jar_dir: arch_harness::archunit::resolve_jar_dir(jar_dir.as_deref()),
                runner_cache: arch_harness::archunit::default_runner_cache(),
                timeout: std::time::Duration::from_secs(
                    timeout_secs.unwrap_or(arch_harness::archunit::DEFAULT_GATE_TIMEOUT_SECS),
                ),
            };
            let outcome = arch_harness::archunit::run_gate(&spec, &opts)?;
            let severity_of = |rule_id: &str| {
                spec.rules
                    .iter()
                    .find(|r| r.id == rule_id)
                    .map_or("error", |r| r.severity.as_str())
            };
            let passed = outcome
                .violations
                .iter()
                .all(|v| severity_of(&v.rule_id) != "error");
            if json {
                let report = serde_json::json!({
                    "repo": repo,
                    "passed": passed,
                    "rules_executed": outcome.rules_executed,
                    "violations": outcome.violations.iter().map(|v| serde_json::json!({
                        "rule": v.rule_id,
                        "severity": severity_of(&v.rule_id),
                        "detail": v.detail,
                    })).collect::<Vec<_>>(),
                    "unsupported": spec.unsupported,
                });
                println!("{report}");
            } else {
                println!(
                    "ArchUnit-гейт: исполнено правил {}, нарушений {}",
                    outcome.rules_executed,
                    outcome.violations.len()
                );
                for v in &outcome.violations {
                    println!(
                        "  [{}] {} — {}",
                        severity_of(&v.rule_id),
                        v.rule_id,
                        v.detail
                    );
                }
                println!("Итог: {}", if passed { "PASS" } else { "FAIL" });
            }
            if !passed {
                std::process::exit(1);
            }
        }
        #[cfg(feature = "harness")]
        ArchunitCmd::Fetch { jar_dir } => {
            let dest = jar_dir.unwrap_or_else(|| {
                arch_harness::config::Config::home_dir()
                    .join("archunit")
                    .join("lib")
            });
            let fetched = arch_harness::archunit::fetch_jars(&dest).await?;
            println!("jar'ы ArchUnit → {}", dest.display());
            for f in &fetched {
                println!(
                    "  {} {} sha256:{}",
                    f.file,
                    if f.cached {
                        "(кэш)"
                    } else {
                        "(скачан)"
                    },
                    f.sha256
                );
            }
        }
    }
    Ok(())
}

/// `arch-be survey <repo>`: обратное обследование legacy → docs/reverse/survey.md.
fn cmd_survey(repo: &Path, out: Option<&Path>) -> Result<()> {
    let outcome = arch_harness::survey::run(repo, out)?;
    println!(
        "обследование `{}`: {} находок [confirmed], {} секций [gap]",
        outcome.report.repo_name,
        outcome.report.confirmed_count(),
        outcome.report.gap_count()
    );
    println!("карта: {}", outcome.survey_path.display());
    if outcome.notes_created {
        println!(
            "создана заготовка заметок [inferred]: {}",
            outcome.notes_path.display()
        );
    }
    Ok(())
}

/// `arch-be fleet`: аудит флота worktree (дубли/дрейф) и гейт мерджа прогонов.
// В core-сборке остаётся только синхронный аудит (гейт мерджа — за
// worktree-фабрикой сборки `harness`) — async-обёртка едина с полной сборкой.
#[cfg_attr(not(feature = "harness"), allow(clippy::unused_async))]
async fn cmd_fleet(cfg: &Arc<Config>, cmd: FleetCmd) -> Result<()> {
    // В core-сборке живёт только аудит (гейт мерджа — за worktree-фабрикой
    // сборки `harness`); конфиг нужен лишь ему.
    #[cfg(not(feature = "harness"))]
    let _ = cfg;
    match cmd {
        FleetCmd::Audit {
            paths,
            repo,
            include,
            format,
            fail_on_dupes,
        } => {
            let mut roots = paths;
            if let Some(repo) = repo {
                roots.extend(arch_harness::fleet::worktrees_from_git(&repo)?);
            }
            let report = arch_harness::fleet::audit(&roots, &include)?;
            match format.as_str() {
                "json" => println!("{}", serde_json::to_string_pretty(&report)?),
                "text" => print!("{}", arch_harness::fleet::render_text(&report)),
                other => anyhow::bail!("неизвестный формат '{other}' (допустимы: text, json)"),
            }
            // Независимые триггеры гейта: дрейф копий и порог доли дублей.
            let dupes_failed = fail_on_dupes.is_some_and(|pct| report.dup_pct > pct);
            if let Some(pct) = fail_on_dupes {
                if dupes_failed {
                    println!(
                        "Порог дублей превышен: {:.1}% > {pct:.1}% — exit 1",
                        report.dup_pct
                    );
                }
            }
            if report.has_drift || dupes_failed {
                std::process::exit(1);
            }
        }
        #[cfg(feature = "harness")]
        FleetCmd::Merge {
            run_id,
            owner_approve,
            repo,
        } => {
            let repo = match repo {
                Some(r) => r,
                None => std::env::current_dir().context("cwd")?,
            };
            match arch_harness::worktree::gated_merge(cfg, &repo, &run_id, owner_approve).await? {
                arch_harness::worktree::MergeGateOutcome::Refused(summary) => {
                    println!("{summary}");
                    std::process::exit(1);
                }
                arch_harness::worktree::MergeGateOutcome::Merged(msg) => println!("{msg}"),
            }
        }
    }
    Ok(())
}

/// `arch-be worktree`: изоляция агентной работы (создание, review, accept, drop).
#[cfg(feature = "harness")]
async fn cmd_worktree(cfg: &Arc<Config>, cmd: WorktreeCmd) -> Result<()> {
    let cwd = std::env::current_dir().context("cwd")?;
    let repo_of = |repo: Option<PathBuf>| repo.unwrap_or_else(|| cwd.clone());
    match cmd {
        WorktreeCmd::New { name, repo, base } => {
            let path =
                arch_harness::worktree::create(cfg, &repo_of(repo), &name, base.as_deref()).await?;
            println!("worktree создан: {}", path.display());
            println!(
                "review: arch-be worktree diff {name} · accept: arch-be worktree accept {name} · drop: arch-be worktree drop {name}"
            );
        }
        WorktreeCmd::List { repo } => {
            let infos = arch_harness::worktree::list(&repo_of(repo)).await?;
            print!("{}", arch_harness::worktree::render_list(&infos));
        }
        WorktreeCmd::Diff { name, repo } => {
            println!(
                "{}",
                arch_harness::worktree::diff(&repo_of(repo), &name).await?
            );
        }
        WorktreeCmd::Accept { name, repo } => {
            println!(
                "{}",
                arch_harness::worktree::accept(cfg, &repo_of(repo), &name).await?
            );
        }
        WorktreeCmd::Drop { name, repo } => {
            println!(
                "{}",
                arch_harness::worktree::drop(cfg, &repo_of(repo), &name).await?
            );
        }
    }
    Ok(())
}

/// `arch-be init`: конфиг + ассеты в ~/.arch-harness.
fn cmd_init(cfg: &Config) -> Result<()> {
    let home = Config::home_dir();
    std::fs::create_dir_all(&home).context("создание домашнего каталога")?;
    let written = arch_harness::assets::write_defaults(&home)?;
    let cfg_path = cfg.save_default()?;
    println!("Инициализация завершена:");
    println!("  конфиг:  {}", cfg_path.display());
    println!("  домашний каталог: {}", home.display());
    for f in &written {
        println!("  ассет:   {}", f.display());
    }
    Ok(())
}

/// Опции headless-прогона `arch-be run` (бюджеты).
#[cfg(feature = "harness")]
struct RunOptions {
    /// Общий таймаут прогона, секунды.
    timeout_secs: Option<u64>,
    /// Переопределение `agent.max_tool_turns` на прогон.
    max_turns: Option<u64>,
}

/// `arch-be run`: headless агент.
///
/// Строгий режим (`--quiet`, как `dsh --profile headless` у `DeepSeek`
/// Harness) = без стриминга: stdout несёт ТОЛЬКО финальный ответ ассистента
/// (пригоден для пайпов), события хода молчат; пустая задача отклоняется
/// до запуска; сбой — причина в stderr и ненулевой код выхода.
///
/// Бюджеты: `--timeout SECS` — общий потолок прогона (превышение — причина
/// в stderr и exit 1), `--max-turns N` — лимит итераций инструментов
/// (перекрывает `agent.max_tool_turns` на клоне конфига).
///
/// В стрим-режиме stdout несёт только текст ответа (дельты); прогресс
/// (вызовы инструментов, заметки) уходит в stderr — пайп остаётся чистым.
#[cfg(feature = "harness")]
async fn cmd_run(
    cfg: &Arc<Config>,
    prompt: Option<String>,
    model: Option<String>,
    stream: bool,
    think: Option<String>,
    opts: RunOptions,
) -> Result<()> {
    let input = match prompt.as_deref() {
        Some("-") | None if !std::io::stdin().is_terminal() => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("чтение stdin")?;
            buf
        }
        Some(p) => p.to_string(),
        None => anyhow::bail!("нет промпта: передайте аргумент или пайп в stdin"),
    };
    if input.trim().is_empty() {
        anyhow::bail!("пустая задача: передайте непустой промпт аргументом или пайпом в stdin");
    }
    let thinking = match think.as_deref() {
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(other) => anyhow::bail!("--think: ожидается on|off, получено '{other}'"),
        None => None,
    };

    // Переопределение бюджета итераций — на клоне конфига, глобальный не трогаем.
    let cfg = match opts.max_turns {
        Some(n) => {
            let mut owned = (**cfg).clone();
            owned.agent.max_tool_turns =
                usize::try_from(n).context("--max-turns: значение не помещается в usize")?;
            Arc::new(owned)
        }
        None => cfg.clone(),
    };

    let registry = Arc::new(LlmRegistry::from_config(&cfg)?);
    let provider = match &model {
        Some(name) => registry.get(name)?,
        None => registry.default(),
    };
    let tools = arch_harness::tools::full_registry(&cfg);
    let cwd = std::env::current_dir().context("cwd")?;
    let tool_ctx = ToolContext::new(cwd, cfg.clone())
        .with_llm(registry.clone())
        .with_provider(provider.clone())
        .with_subagents(arch_harness::subagent::SubagentRegistry::new());
    let system = default_system_prompt(&cfg);
    let mut session = AgentSession::new(cfg.clone(), provider, tools, tool_ctx, system);
    session.set_thinking(thinking);

    let send = async {
        if stream {
            let (tx, mut rx) = tokio::sync::mpsc::channel(64);
            let printer = tokio::spawn(async move {
                use arch_harness::agent::AgentEvent;
                while let Some(ev) = rx.recv().await {
                    // StdoutLock/StderrLock не Send — лочим на каждое событие,
                    // не через await.
                    match ev {
                        AgentEvent::Delta(text) => {
                            let mut out = std::io::stdout().lock();
                            let _ = out.write_all(text.as_bytes());
                            let _ = out.flush();
                        }
                        // «Мысли» — приглушённо в stderr (прогресс-канал).
                        AgentEvent::ReasoningDelta(text) => {
                            let mut err = std::io::stderr().lock();
                            let _ = write!(err, "\x1b[2m{text}\x1b[0m");
                            let _ = err.flush();
                        }
                        // Прогресс — в stderr: stdout пайпа несёт только ответ.
                        AgentEvent::ToolStart { name, .. } => {
                            let _ =
                                writeln!(std::io::stderr().lock(), "\x1b[2m▶ tool: {name}\x1b[0m");
                        }
                        AgentEvent::ToolEnd {
                            name,
                            is_error,
                            summary,
                            ..
                        } => {
                            let mark = if is_error { "✗" } else { "✓" };
                            let _ = writeln!(
                                std::io::stderr().lock(),
                                "\x1b[2m{mark} {name}: {summary}\x1b[0m"
                            );
                        }
                        AgentEvent::Note(text) => {
                            let _ = writeln!(std::io::stderr().lock(), "\x1b[2m» {text}\x1b[0m");
                        }
                        AgentEvent::TurnDone => {
                            let _ = writeln!(std::io::stdout().lock());
                        }
                        // Телеметрия индикатора контекста — только для TUI.
                        AgentEvent::ContextUsage(_) => {}
                    }
                }
            });
            let r = session.send(&input, Some(tx)).await;
            let _ = printer.await;
            r.map_err(anyhow::Error::from)
        } else {
            session
                .send(&input, None)
                .await
                .map_err(anyhow::Error::from)
        }
    };
    let reply = match opts.timeout_secs {
        Some(secs) => match tokio::time::timeout(Duration::from_secs(secs), send).await {
            Ok(r) => r?,
            Err(_) => anyhow::bail!(
                "таймаут прогона ({secs}с): провайдер или инструмент не ответил вовремя"
            ),
        },
        None => send.await?,
    };
    if !stream {
        // Печать через writeln с игнорированием BrokenPipe: `arch-be run -q … | head`
        // обрывает stdout — для пайпа это норма, а не повод для паники println!.
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
    Ok(())
}

/// Системный промпт по умолчанию: из библиотеки промптов или встроенный,
/// дополненный глобальной md-памятью (`paths.memory_file`, см. `memory`).
#[cfg(feature = "harness")]
fn default_system_prompt(cfg: &Config) -> String {
    let dir = cfg.paths.prompts_dir();
    let base = match arch_harness::agent::prompts::load_library(&dir) {
        Ok(lib) => match lib.iter().find(|t| t.name == "architect") {
            Some(tpl) => tpl.body.clone(),
            None => fallback_system_prompt(),
        },
        Err(_) => fallback_system_prompt(),
    };
    // Ошибка чтения памяти не фатальна: сессия работает без неё.
    let memory = arch_harness::memory::load(&cfg.paths.memory_file)
        .ok()
        .flatten();
    arch_harness::memory::augment_system_prompt(&base, memory.as_deref(), &cfg.paths.memory_file)
}

/// Встроенный системный промпт (fallback, когда библиотека недоступна).
#[cfg(feature = "harness")]
fn fallback_system_prompt() -> String {
    "Ты — solution-архитектор в корпоративном контуре банка. Помогаешь проектировать \
     решения, ведёшь ADR и architecture-spine, оцениваешь архитектуру по рубрикам, \
     готовишь handoff-пакеты кодовым агентам. Отвечай по-русски, точно и по делу."
        .into()
}

/// `arch-be prompts` (только сборка `harness`).
#[cfg(feature = "harness")]
fn cmd_prompts(cfg: &Config, name: Option<String>) -> Result<()> {
    let lib = arch_harness::agent::prompts::load_library(&cfg.paths.prompts_dir())?;
    match name {
        None => {
            println!(
                "Библиотека промптов ({}):",
                cfg.paths.prompts_dir().display()
            );
            for tpl in &lib {
                println!("  {:<24} {}", tpl.name, tpl.description);
            }
        }
        Some(n) => {
            let tpl = lib
                .iter()
                .find(|t| t.name == n)
                .with_context(|| format!("шаблон '{n}' не найден"))?;
            println!("{}", tpl.body);
        }
    }
    Ok(())
}

/// `arch-be memory [add <текст>]`: путь и содержимое глобальной md-памяти
/// либо дописка заметки в конец файла.
fn cmd_memory(cfg: &Config, cmd: Option<MemoryCmd>) -> Result<()> {
    let path = &cfg.paths.memory_file;
    match cmd {
        None => match arch_harness::memory::load(path)? {
            Some(content) => println!("Память ({}):\n{content}", path.display()),
            None => println!(
                "память пустая, файл: {} (дописать — arch-be memory add <текст>)",
                path.display()
            ),
        },
        Some(MemoryCmd::Add { text }) => {
            arch_harness::memory::append(path, &text)?;
            println!("заметка дописана в память: {}", path.display());
        }
    }
    Ok(())
}

/// Прокси к Archify CLI (`node <archify.mjs> …`): печатает вывод, код
/// возврата CLI становится кодом возврата `arch-be` (гейт для CI/скриптов).
async fn cmd_archify(cfg: &Config, cmd: ArchifyCmd) -> Result<()> {
    let cwd = std::env::current_dir().context("archify: не удалось определить рабочий каталог")?;
    let args: Vec<String> = match &cmd {
        ArchifyCmd::Doctor => vec!["doctor".into()],
        ArchifyCmd::Guide { query } => vec!["guide".into(), query.clone(), "--json".into()],
        ArchifyCmd::Validate {
            r#type,
            path,
            quality,
            json: _,
        } => vec![
            "validate".into(),
            r#type.clone(),
            path.to_string_lossy().into_owned(),
            "--quality".into(),
            quality.clone(),
            "--json".into(),
        ],
        ArchifyCmd::Deliver {
            r#type,
            path,
            output,
            quality,
            json: _,
        } => vec![
            "deliver".into(),
            r#type.clone(),
            path.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
            "--quality".into(),
            quality.clone(),
            "--json".into(),
        ],
        ArchifyCmd::Compare {
            base,
            head,
            output,
            quality,
            json: _,
        } => vec![
            "compare".into(),
            "architecture".into(),
            base.to_string_lossy().into_owned(),
            head.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
            "--quality".into(),
            quality.clone(),
            "--json".into(),
        ],
    };
    let run = arch_harness::archify::run(cfg, &cwd, &args, cfg.archify.timeout_secs).await?;
    // Для validate/deliver/compare отдаём компактную сводку receipt;
    // doctor/guide — сырой вывод CLI (текст/JSON рекомендации).
    // Флаг --json: сырой JSON-receipt Archify CLI (SDK-контракт v1).
    let json_mode = matches!(
        cmd,
        ArchifyCmd::Validate { json: true, .. }
            | ArchifyCmd::Deliver { json: true, .. }
            | ArchifyCmd::Compare { json: true, .. }
    );
    let summarize = !matches!(cmd, ArchifyCmd::Doctor | ArchifyCmd::Guide { .. });
    if json_mode {
        print!("{}", run.stdout);
    } else if summarize {
        print!(
            "{}",
            arch_harness::archify::summarize_receipt(&args[0], &run.stdout)
        );
    } else {
        print!("{}", run.stdout);
    }
    if !run.stderr.trim().is_empty() {
        eprint!("{}", run.stderr);
    }
    if run.timed_out {
        anyhow::bail!(
            "archify {}: таймаут {} сек",
            args[0],
            cfg.archify.timeout_secs
        );
    }
    if !run.ok() {
        anyhow::bail!(
            "archify {}: провал (код выхода {})",
            args[0],
            run.status.map_or("?".to_string(), |c| c.to_string())
        );
    }
    Ok(())
}

/// Обрабатывает `arch-be rules …`: кандидаты и шаблоны исполняемых правил.
fn cmd_rules(cmd: RulesCmd) -> Result<()> {
    match cmd {
        RulesCmd::Suggest { path } => {
            let report = arch_harness::rules_suggest::suggest(&path)?;
            print!("{}", arch_harness::rules_suggest::render_markdown(&report));
            Ok(())
        }
        RulesCmd::Template { cmd } => cmd_rules_template(cmd),
    }
}

/// Обрабатывает `arch-be rules template …`.
fn cmd_rules_template(cmd: RulesTemplateCmd) -> Result<()> {
    use arch_harness::rule_templates as rt;
    match cmd {
        RulesTemplateCmd::List => {
            print!("{}", rt::render_list()?);
            Ok(())
        }
        RulesTemplateCmd::Show { id } => {
            print!("{}", rt::render_show(&id)?);
            Ok(())
        }
        RulesTemplateCmd::Apply {
            id,
            ad,
            dir,
            lang,
            dry_run,
        } => {
            let lang = rt::Lang::parse(&lang)?;
            let report = rt::apply(&dir, &id, &ad, lang, dry_run)?;
            println!(
                "Шаблон: {} v{} → {}",
                report.template,
                report.version,
                report.target_dir.display()
            );
            println!(
                "Файлов {}: {}",
                if report.dry_run {
                    "было бы записано"
                } else {
                    "записано"
                },
                report.written.len()
            );
            for path in &report.written {
                println!("  {}", path.display());
            }
            println!(
                "\nФрагмент для CONSTRAINTS.yaml (под ключом `rules:`) — {}:\n",
                if report.dry_run {
                    "печатается, на диск не пишется"
                } else {
                    "печатается, НЕ вносится"
                }
            );
            println!("{}", report.fragment);
            println!(
                "\nСтрока для сущности инварианта в model/ (вторая строка frontmatter):\n  {}",
                report.verified_by
            );
            if !report.notes.is_empty() {
                println!("\nЗамечания:");
                for note in &report.notes {
                    println!("  - {note}");
                }
            }
            Ok(())
        }
        RulesTemplateCmd::Verify {
            all,
            dir,
            java_jar,
            lang,
            require_python,
        } => {
            let lang = rt::Lang::parse(&lang)?;
            let runner = rt::Runner::detect(java_jar.as_deref());
            let report = match (all, dir) {
                (true, None) => rt::verify_all(&runner, lang, require_python)?,
                (false, Some(case)) => rt::verify_dir(&case, &runner, lang)?,
                _ => {
                    return Err(anyhow::Error::msg(
                        "укажите ровно одно: --all (шаблоны библиотеки) или --dir <кейс>",
                    ));
                }
            };
            print!("{}", rt::render_verify(&report));
            if !report.passed() {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

async fn cmd_rubric(cfg: &Arc<Config>, cmd: RubricCmd) -> Result<()> {
    match cmd {
        RubricCmd::List => {
            let list = arch_harness::rubric::list(&cfg.paths.rubrics_dir())?;
            for r in &list {
                println!(
                    "  {:<32} {} ({} критериев)",
                    r.name, r.description, r.criteria_count
                );
            }
        }
        RubricCmd::Run {
            rubric,
            target,
            pack,
            subject,
            root,
            model,
            dynamic_subject,
            author_model,
        } => {
            let registry = Arc::new(LlmRegistry::from_config(cfg)?);
            let judge = match &model {
                Some(name) => registry.get(name)?,
                None => registry.default(),
            };
            // Досье (ADR-051): субъект и вид заданы — собираем из репозитория
            // и судим с проверкой цитат по ролям; иначе обычный документ.
            let dossier = match (&pack, &subject) {
                (Some(kind), Some(subject)) => {
                    let repo = root.clone().unwrap_or_else(|| PathBuf::from("."));
                    let repo = repo.canonicalize().unwrap_or(repo);
                    let kind = arch_harness::rubric_pack::PackKind::parse(kind)?;
                    let packs = arch_harness::rubric_pack::build(&repo, kind, subject)?;
                    if packs.len() > 1 {
                        anyhow::bail!(
                            "досье дробится на {} фрагментов — вызывайте по каждому, \
                             указав субъект с диапазоном строк (например, '{subject}#1-40')",
                            packs.len()
                        );
                    }
                    Some(packs.into_iter().next().expect("один фрагмент"))
                }
                (None, None) => None,
                _ => {
                    anyhow::bail!("`--pack` и `--subject` задаются вместе: вид досье и его субъект")
                }
            };
            if dossier.is_some() && target.is_some() {
                anyhow::bail!(
                    "`--pack`/`--subject` несовместимы с целевым документом: досье \
                     собирается из репозитория"
                );
            }
            let text = match (&dossier, &target) {
                (Some(pack), _) => pack.text.clone(),
                (None, Some(target)) => std::fs::read_to_string(target)
                    .with_context(|| format!("чтение {}", target.display()))?,
                (None, None) => anyhow::bail!(
                    "укажите целевой документ или `--pack`/`--subject` (смысловая рубрика)"
                ),
            };
            let rub = if let Some(subject) = dynamic_subject {
                let anchor_path = resolve_asset(&cfg.paths.rubrics_dir(), &rubric, "yaml");
                let anchor = arch_harness::rubric::load(&anchor_path).ok();
                arch_harness::rubric::generate_dynamic(&subject, anchor.as_ref(), judge.as_ref())
                    .await?
            } else {
                let path = resolve_asset(&cfg.paths.rubrics_dir(), &rubric, "yaml");
                arch_harness::rubric::load(&path)?
            };
            let report = match &dossier {
                Some(pack) => {
                    arch_harness::rubric::evaluate_pack(&rub, pack, judge.as_ref(), &cfg.judge)
                        .await?
                }
                None => arch_harness::rubric::evaluate(&rub, &text, judge.as_ref()).await?,
            };
            println!("{}", report.to_markdown());
            let out = cfg
                .paths
                .reports_dir
                .join(format!("rubric-{}-{}.md", rub.name, timestamp()));
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&out, report.to_markdown())?;
            eprintln!("Отчёт: {}", out.display());
            // Машиночитаемый отчёт (Н7, ADR-042; досье — ADR-051): его читает
            // гейт. Пишем в репозиторий, к которому относится вход, — иначе
            // гейт его не найдёт.
            let written = match (&dossier, &target) {
                (Some(pack), _) => {
                    let repo = root
                        .clone()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .canonicalize()
                        .unwrap_or_else(|_| PathBuf::from("."));
                    arch_harness::rubric::write_artifact_for_subject(
                        &repo,
                        &report,
                        &arch_harness::rubric::ArtifactSubject::Pack(pack),
                        author_model.as_deref(),
                    )
                }
                (None, Some(target)) => {
                    let abs = target.canonicalize().unwrap_or_else(|_| target.clone());
                    let repo = arch_harness::rubric::repo_root_of(&abs);
                    arch_harness::rubric::write_artifact(
                        &repo,
                        &report,
                        Some(&abs),
                        author_model.as_deref(),
                    )
                }
                (None, None) => unreachable!("вход проверен выше"),
            };
            match written {
                Ok(path) => eprintln!("Отчёт для гейта: {}", path.display()),
                Err(e) => eprintln!("⚠ машиночитаемый отчёт не записан: {e}"),
            }
        }
        RubricCmd::Pack {
            kind,
            subject,
            root,
        } => {
            let repo = root.unwrap_or_else(|| PathBuf::from("."));
            let repo = repo.canonicalize().unwrap_or(repo);
            let kind = arch_harness::rubric_pack::PackKind::parse(&kind)?;
            let packs = arch_harness::rubric_pack::build(&repo, kind, &subject)?;
            for p in &packs {
                println!("{}", p.text);
                println!();
                println!(
                    "досье '{}' · субъект '{}' · sha256:{} · источников {} (ссылочных {})",
                    p.kind.as_str(),
                    p.subject,
                    p.sha256,
                    p.inputs.len(),
                    p.references().len()
                );
                for i in &p.inputs {
                    println!(
                        "  [{}] {} {}",
                        i.role.as_str(),
                        i.path,
                        i.id.as_deref().unwrap_or("-")
                    );
                }
            }
            eprintln!(
                "Досье собрано: {} (субъект '{}', вид '{}')",
                packs.len(),
                subject,
                kind.as_str()
            );
        }
    }
    Ok(())
}

/// `arch-be bench` (только сборка `harness`).
#[cfg(feature = "harness")]
async fn cmd_bench(cfg: &Arc<Config>, cmd: BenchCmd) -> Result<()> {
    match cmd {
        BenchCmd::List => {
            for b in arch_harness::bench::list(&cfg.paths.benchmarks_dir())? {
                println!("  {:<32} {} [{}]", b.name, b.description, b.tags.join(", "));
            }
        }
        BenchCmd::Run {
            name,
            model,
            golden,
            rubric,
            record,
        } => {
            if record.is_some() && !golden {
                anyhow::bail!("`bench run --record` применим только с --golden");
            }
            if rubric.is_some() && !golden {
                anyhow::bail!("`bench run --rubric` применим только с --golden");
            }
            let registry = LlmRegistry::from_config(cfg)?;
            let provider = match &model {
                Some(m) => registry.get(m)?,
                None => registry.default(),
            };
            if golden {
                if name.is_some() {
                    anyhow::bail!("`bench run --golden` не совместим с именем бенчмарка");
                }
                // Регрессионный гейт качества судьи (ADR-004): согласие с
                // эталоном ниже порога — exit 1, как у `control check`.
                let report = arch_harness::bench::run_golden_filtered(
                    provider.as_ref(),
                    &cfg.paths.rubrics_dir(),
                    &cfg.paths.benchmarks_dir().join("golden"),
                    &cfg.judge,
                    rubric.as_deref(),
                )
                .await?;
                println!(
                    "Golden-прогон судьи '{}' (сэмплов на критерий: {}):",
                    report.judge_model, cfg.judge.samples
                );
                for case in &report.cases {
                    println!(
                        "  {:<32} MAE {:.2} ({} критериев)",
                        case.doc, case.mae, case.compared
                    );
                }
                // Механическая диагностика: MAE по критериям и length bias.
                print!("{}", report.diagnostics_text());
                let passed = report.mae <= cfg.judge.golden_max_mae;
                println!(
                    "Итог MAE: {:.2} по {} парам (порог {:.2}) — {}",
                    report.mae,
                    report.compared,
                    cfg.judge.golden_max_mae,
                    if passed { "PASS" } else { "FAIL" }
                );
                // Evidence-журнал (M-2): запись не зависит от исхода гейта —
                // история хранит и регрессии.
                if let Some(path) = &record {
                    let entry = arch_harness::bench::GoldenRecord::from_report(
                        &report,
                        chrono::Local::now().format("%Y-%m-%d").to_string(),
                    );
                    arch_harness::bench::record_golden(path, &entry)?;
                    println!("Записано в журнал: {}", path.display());
                }
                if !passed {
                    std::process::exit(1);
                }
                return Ok(());
            }
            let Some(name) = name else {
                anyhow::bail!("укажите имя бенчмарка или флаг --golden");
            };
            let path = resolve_asset(&cfg.paths.benchmarks_dir(), &name, "yaml");
            let bench = arch_harness::bench::load(&path)?;
            let report = arch_harness::bench::run(
                &bench,
                provider.as_ref(),
                &cfg.paths.rubrics_dir(),
                &cfg.paths.reports_dir,
                &cfg.judge,
            )
            .await?;
            println!(
                "Бенчмарк '{}': {:.2} (порог {:.2}) — {}",
                report.bench_name,
                report.rubric_report.weighted_total,
                bench.pass_threshold,
                if report.passed { "PASS" } else { "FAIL" }
            );
        }
        BenchCmd::GoldenHistory { path } => {
            let (records, broken) = arch_harness::bench::load_golden_history(&path)?;
            if broken > 0 {
                eprintln!("пропущено битых строк: {broken}");
            }
            if records.is_empty() {
                println!("Журнал {} пуст.", path.display());
            } else {
                print!("{}", arch_harness::bench::golden_history_markdown(&records));
            }
        }
        BenchCmd::HumanAgreement { golden_dir, humans } => {
            let report = arch_harness::bench::human_agreement(&golden_dir, &humans)?;
            for doc in &report.skipped {
                eprintln!("пропущен {doc}: нет человеческих анкет");
            }
            print!("{}", report.to_markdown());
        }
    }
    Ok(())
}

/// `arch-be web` (только сборка `harness`).
#[cfg(feature = "harness")]
async fn cmd_web(cfg: &Config, cmd: WebCmd) -> Result<()> {
    match cmd {
        WebCmd::Search { query, arch } => {
            let results = if arch {
                arch_harness::web::search_arch_sites(&query, &[], &cfg.web).await?
            } else {
                arch_harness::web::search(&query, &cfg.web).await?
            };
            for r in &results {
                println!("• {}\n  {}\n  {}\n", r.title, r.url, r.snippet);
            }
            if results.is_empty() {
                println!("Ничего не найдено.");
            }
        }
        WebCmd::Fetch { url } => {
            let text = arch_harness::web::fetch(&url, &cfg.web).await?;
            println!("{text}");
        }
        WebCmd::Sites => {
            println!("Кураторские сайты архитектора:");
            for s in arch_harness::web::curated_sites(&cfg.web) {
                println!("  {:<16} {:<40} {}", s.name, s.base_url, s.description);
            }
        }
    }
    Ok(())
}

async fn cmd_mcp(cfg: &Arc<Config>, cmd: McpCmd) -> Result<()> {
    // Серверный режим (P1-2, ADR-008) обслуживает клиентов и не подключается
    // к серверам: mcp.json для него не требуется, уходим до его загрузки.
    if let McpCmd::Serve { rw } = &cmd {
        let mode = if *rw {
            arch_harness::mcp_server::ServeMode::ReadWrite
        } else {
            arch_harness::mcp_server::ServeMode::ReadOnly
        };
        return arch_harness::mcp_server::serve_with_mode(Arc::clone(cfg), mode)
            .await
            .context("MCP-сервер (stdio)");
    }
    let mut servers = arch_harness::mcp::load_servers(&cfg.mcp.servers_file)
        .with_context(|| format!("чтение {}", cfg.mcp.servers_file.display()))?;
    // Плагины тоже несут MCP-серверы (стандарт: plugin.json mcpServers / .mcp.json).
    if cfg.plugins.include_mcp {
        let plugins = arch_harness::plugin::discover(&cfg.plugins.dirs);
        servers.extend(arch_harness::plugin::mcp_servers(&plugins));
    }
    let manager =
        Arc::new(arch_harness::mcp::McpManager::connect(&servers, cfg.mcp.timeout_secs).await?);
    match cmd {
        McpCmd::List => {
            println!("Серверы: {}", manager.server_names().join(", "));
            for spec in manager.tools().await {
                println!("  {:<40} {}", spec.name, spec.description);
            }
        }
        McpCmd::Call { name, args } => {
            let args: serde_json::Value =
                serde_json::from_str(&args).context("невалидный JSON аргументов")?;
            let out = manager.call(&name, args).await?;
            println!("{}", out.content);
        }
        // Недостижимо: Serve обработан выше возвратом до подключения к серверам.
        McpCmd::Serve { .. } => {}
    }
    manager.shutdown().await;
    Ok(())
}

/// `arch-be adr`: реестр ADR по набору проектов (ADR-036).
fn cmd_adr(cmd: AdrCmd) -> Result<()> {
    match cmd {
        AdrCmd::Registry { root, json, strict } => {
            let report = arch_harness::adr_registry::build_registry(&root)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("RegistryReport сериализуется")
                );
            } else {
                println!("{}", arch_harness::adr_registry::render_markdown(&report));
            }
            let code = arch_harness::adr_registry::exit_code(&report, strict);
            if code != 0 {
                std::process::exit(code);
            }
        }
    }
    Ok(())
}

/// Публикация артефактов в корпоративные системы (файловые адаптеры, ADR-033).
fn cmd_publish(cmd: PublishCmd) -> Result<()> {
    match cmd {
        PublishCmd::Confluence { file } => {
            let md = std::fs::read_to_string(&file)
                .map_err(|e| arch_harness::error::HarnessError::io(&file, e))?;
            println!("{}", arch_harness::publish::markdown_to_confluence(&md));
        }
        PublishCmd::Jira { result, project } => {
            print!(
                "{}",
                arch_harness::publish::handoff_json_to_jira_csv(&result, project.as_deref())?
            );
        }
    }
    Ok(())
}

/// Единый резолвер реестра ограничений для CLI-арм (E2): явный путь →
/// пакетная копия (`.arch-handoff/CONSTRAINTS.yaml`) → корневая
/// (`CONSTRAINTS.yaml`); ни одной копии — канонический дефолт, чтобы
/// ошибка «файл не читается» ссылалась на пакетный путь.
fn resolve_constraints_cli(repo: &Path, explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| {
        arch_harness::control::resolve_constraints_path(repo, None)
            .unwrap_or_else(|| repo.join(arch_harness::control::HANDOFF_CONSTRAINTS_PATH))
    })
}

fn cmd_control(cfg: &arch_harness::config::Config, cmd: ControlCmd) -> Result<()> {
    match cmd {
        ControlCmd::Check {
            repo,
            constraints,
            json,
            baseline,
            baseline_update,
            changed_since,
            format,
            base,
        } => {
            let explicit = constraints.is_some();
            let c = resolve_constraints_cli(&repo, constraints);
            // T-01: реестра нет ни в корне, ни в `.arch-handoff/` — раньше
            // сюда улетал сырой `io: ./.arch-handoff/CONSTRAINTS.yaml: No such
            // file` (адрес, которого пользователь не выбирал). Pre-commit-хук
            // читает этот текст, поэтому причина называется прямо.
            if !explicit && !c.is_file() {
                anyhow::bail!(
                    "реестр правил не найден: ни {} в корне, ни {} — создайте каркас: `arch-be bootstrap`",
                    arch_harness::control::ROOT_CONSTRAINTS_PATH,
                    arch_harness::control::HANDOFF_CONSTRAINTS_PATH
                );
            }
            let options = arch_harness::control::baseline::CheckOptions {
                baseline,
                baseline_update,
                changed_since,
            };
            // П5: сверка состава правил с git-базой — «правило выполняется»
            // плюс «правило ещё существует» в любом канале, не только в gate.
            let report =
                arch_harness::control::check_anchored(&repo, &c, &options, base.as_deref())?;
            let format = arch_harness::report_fmt::ReportFormat::parse(&format)
                .map_err(anyhow::Error::msg)?;
            if json {
                // SDK-контракт v1: машиночитаемый отчёт, exit code как в текстовом режиме.
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("FitnessReport сериализуется")
                );
            } else if format != arch_harness::report_fmt::ReportFormat::Text {
                // Машинные форматы CI (SARIF/JUnit/GitLab Code Quality/markdown):
                // строго в stdout, exit-код как у текста.
                print!(
                    "{}",
                    arch_harness::report_fmt::render(
                        format,
                        &arch_harness::report_fmt::FmtReport::from_fitness(&report),
                    )
                );
            } else {
                println!("{}", report.summary);
                // Наследование корп-спайна (extends): метки источников видны
                // в выводе (docs/corp-spine.md).
                if !report.inherited.is_empty() {
                    let sources = report
                        .inherited
                        .iter()
                        .map(|s| format!("{} ({})", s.source, s.rules))
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("Источники правил: {sources}");
                }
                for o in &report.overrides {
                    println!(
                        "  [override:{}] {} (adr {}, until {}) — {}",
                        o.status, o.rule, o.adr, o.until, o.note
                    );
                }
                for i in &report.issues {
                    println!(
                        "  [{}] {}:{} {} — {}",
                        i.severity,
                        i.file.display(),
                        i.line,
                        i.rule,
                        i.message
                    );
                    // Карточный контекст правила — одной строкой-отступом и
                    // только при наличии rationale/fix_hint (не раздуваем).
                    if i.rationale.is_some() || i.fix_hint.is_some() {
                        let mut parts: Vec<String> = Vec::new();
                        if let Some(ad) = &i.ad {
                            parts.push(ad.clone());
                        }
                        if let Some(adr) = &i.adr {
                            parts.push(adr.clone());
                        }
                        if let Some(rationale) = &i.rationale {
                            parts.push(format!("зачем: {rationale}"));
                        }
                        if let Some(fix_hint) = &i.fix_hint {
                            parts.push(format!("как чинить: {fix_hint}"));
                        }
                        if let Some(skill) = &i.skill {
                            parts.push(format!("скилл: {skill}"));
                        }
                        if let Some(owner) = &i.owner {
                            parts.push(format!("владелец: {owner}"));
                        }
                        println!("      ↳ {}", parts.join(" · "));
                    }
                }
                // Режим ratchet: долг по правилам и владельцам + закрытые
                // находки (долг не попадает в список issues выше).
                if let Some(baseline_report) = &report.baseline {
                    print!(
                        "{}",
                        arch_harness::control::baseline::render_baseline_section(baseline_report)
                    );
                }
                // Режим --changed-since: размер среза и пропущенные правила.
                if let Some(reference) = &report.changed_since {
                    print!(
                        "{}",
                        arch_harness::control::baseline::render_scope_section(
                            reference,
                            report.changed_files.unwrap_or(0),
                            &report.skipped,
                        )
                    );
                }
                // Топ-5 самых медленных правил — только если есть правила > 1s.
                let mut slow: Vec<&arch_harness::control::RuleDuration> =
                    report.durations.iter().filter(|d| d.ms > 1000).collect();
                slow.sort_by(|a, b| b.ms.cmp(&a.ms).then(a.rule.cmp(&b.rule)));
                if !slow.is_empty() {
                    println!("Самые медленные правила:");
                    for d in slow.iter().take(5) {
                        println!("  {:.1}s {}", d.ms as f64 / 1000.0, d.rule);
                    }
                }
                println!("Итог: {}", if report.passed { "PASS" } else { "FAIL" });
            }
            if !report.passed {
                std::process::exit(1);
            }
        }
        ControlCmd::Spine { file } => {
            let issues = arch_harness::control::lint_spine(&file)?;
            if issues.is_empty() {
                println!("spine: нарушений нет");
            }
            for i in &issues {
                println!(
                    "[{}] {}:{} {} — {}",
                    i.severity,
                    i.file.display(),
                    i.line,
                    i.rule,
                    i.message
                );
            }
            // Гейт в CI: error-находки ломают сборку (warn — только отчёт).
            let errors = issues.iter().filter(|i| i.severity == "error").count();
            if !issues.is_empty() {
                println!("Итог: {} находок (error: {errors})", issues.len());
            }
            if errors > 0 {
                std::process::exit(1);
            }
        }
        ControlCmd::Sensors { dir } => {
            let results = arch_harness::control::sensors_check(&dir)?;
            // Провал сенсора — ненулевой exit (годится для CI): до волны
            // DB-гейта команда печатала [FAIL], но завершалась с кодом 0,
            // и дефект формы спецификации проходил контур незамеченным
            // (red-team кейса 011, вариант 06). PASS всех сенсоров — exit 0,
            // поведение зелёных кейсов не меняется.
            let failed = results.iter().filter(|r| !r.passed).count();
            for r in &results {
                println!(
                    "  [{}] {} {} — {}",
                    if r.passed { "PASS" } else { "FAIL" },
                    r.sensor,
                    r.file.display(),
                    r.details
                );
            }
            println!(
                "Итог: {} — сенсоров: {}, провалено: {failed}",
                if failed == 0 { "PASS" } else { "FAIL" },
                results.len()
            );
            if failed > 0 {
                std::process::exit(1);
            }
        }
        ControlCmd::Score { trigger, from_diff } => {
            // Пороги маршрутов — из конфига ([significance], ADR-034);
            // невалидные границы — понятная ошибка при чтении.
            let (fast_max, standard_max) = cfg
                .significance
                .limits()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut answers = std::collections::BTreeMap::new();
            for t in &trigger {
                let (k, v) = t
                    .split_once('=')
                    .with_context(|| format!("триггер '{t}' не вида имя=true"))?;
                answers.insert(k.to_string(), v == "true");
            }
            // T-04: незнакомое имя — ошибка, а не завышенный маршрут. Раньше
            // `--trigger foo=true` попадал в счёт наравне с каноническим.
            let unknown = arch_harness::control::unknown_trigger_names(&answers);
            if !unknown.is_empty() {
                anyhow::bail!(
                    "control score: {}",
                    arch_harness::control::unknown_triggers_error(&unknown)
                );
            }
            if let Some(git_ref) = from_diff {
                // S-1 anti-bypass: «HEAD» (дефолт флага) — рабочее дерево
                // против HEAD; иное значение — GIT_REF...HEAD.
                let git_ref = (git_ref != "HEAD").then_some(git_ref);
                // T-05: глобы детекторов — из `[significance]` конфига.
                let diff = arch_harness::control::detect_diff_triggers_with(
                    std::path::Path::new("."),
                    git_ref.as_deref(),
                    &cfg.significance.diff_globs(),
                )?;
                let scored = arch_harness::control::score_with_sources(
                    &answers,
                    &diff,
                    fast_max,
                    standard_max,
                );
                let fired = scored
                    .significance
                    .fired
                    .iter()
                    .map(|f| {
                        scored
                            .sources
                            .get(f)
                            .map_or_else(|| f.clone(), |s| format!("{f} ({})", s.label()))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "Score: {} ({fired} триггеров) → маршрут {:?}",
                    scored.significance.score, scored.significance.route
                );
                for e in &diff.evidence {
                    println!("  diff: {e}");
                }
                if !scored.undeclared.is_empty() {
                    println!(
                        "ВНИМАНИЕ — расхождение: заявлено флагами vs видно по диффу: {}",
                        scored.undeclared.join(", ")
                    );
                }
            } else {
                let s = arch_harness::control::significance_score_with_limits(
                    &answers,
                    fast_max,
                    standard_max,
                );
                println!(
                    "Score: {} ({} триггеров) → маршрут {:?}",
                    s.score,
                    s.fired.join(", "),
                    s.route
                );
            }
        }
        ControlCmd::RulesReport { repo, constraints } => {
            let resolution = arch_harness::control::resolve_constraints_path_detailed(
                &repo,
                constraints.as_deref(),
            );
            let c = resolution.as_ref().map_or_else(
                || repo.join(arch_harness::control::HANDOFF_CONSTRAINTS_PATH),
                |r| r.path.clone(),
            );
            if let Some(note) = resolution.and_then(|r| r.drift_note()) {
                println!("Внимание: {note}");
            }
            print!("{}", arch_harness::control::rules_report(&repo, &c)?);
        }
        ControlCmd::RulesSuggest { path } => {
            let report = arch_harness::rules_suggest::suggest(&path)?;
            print!("{}", arch_harness::rules_suggest::render_markdown(&report));
        }
        ControlCmd::Report {
            repo,
            constraints,
            level,
            json,
        } => {
            let c = resolve_constraints_cli(&repo, constraints);
            let report = arch_harness::control::control_report(&repo, &c, &level)?;
            if json {
                // SDK-контракт v1: машиночитаемый отчёт; report — отчётность,
                // exit code гейт не дублирует.
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("ControlReport сериализуется")
                );
            } else {
                print!("{}", arch_harness::control::render_control_report(&report));
            }
        }
        ControlCmd::Adr { title, dir } => {
            let dir = dir.unwrap_or_else(|| PathBuf::from("docs/adr"));
            let path = arch_harness::control::adr_new(&dir, &title)?;
            println!("ADR создан: {}", path.display());
        }
        ControlCmd::Gate {
            gate,
            packet,
            rehearse,
            require_rehearsal,
        } => {
            use arch_harness::rehearsal as rh;
            if !gate.eq_ignore_ascii_case("a4") {
                anyhow::bail!("гейт '{gate}' не реализован механически (пока только A4)");
            }
            let requirement: rh::RehearsalRequirement = require_rehearsal
                .parse()
                .map_err(|e: String| anyhow::anyhow!("--require-rehearsal: {e}"))?;
            let (repo, packet_dir) = rh::locate_packet(&packet)?;
            // Н6: неполный пакет — находка архитектурного процесса с тем, что
            // сделать, а не io-ошибка «нет MANIFEST.json».
            if let Some(f) = rh::check_packet(&packet_dir)? {
                println!("[error] {} — {}", f.rule, f.message);
                println!("  → {}", f.fix_hint);
                println!("Итог: FAIL");
                std::process::exit(1);
            }
            let route = rh::packet_route(&packet_dir)?;
            let report = if rehearse {
                let report = rh::rehearse(&repo, &packet_dir)?;
                println!("Репетиция отката (baseline {}):", report.baseline_commit);
                for line in &report.log {
                    println!("  {line}");
                }
                println!(
                    "  evidence: {}",
                    packet_dir.join(rh::REHEARSAL_FILE).display()
                );
                Some(report)
            } else {
                rh::load_report(&packet_dir)?
            };
            // Свежесть evidence сверяем с текущим планом (если он читается).
            let plan_baseline = rh::load_plan(&packet_dir)
                .ok()
                .map(|p| p.baseline_commit.trim().to_string());
            let verdict = rh::gate_a4(
                route,
                requirement,
                plan_baseline.as_deref(),
                report.as_ref(),
            );
            println!("{}", verdict.summary);
            println!("Итог: {}", if verdict.passed { "PASS" } else { "FAIL" });
            if !verdict.passed {
                std::process::exit(1);
            }
        }
        ControlCmd::Fp { cmd } => match cmd {
            FpCmd::Mark {
                rule,
                file,
                note,
                repo,
            } => {
                let repo = repo.unwrap_or_else(|| PathBuf::from("."));
                let path =
                    arch_harness::digest::fp_register_mark(&repo, &rule, &file, note.as_deref())?;
                println!("Пометка FP записана: {}", path.display());
            }
        },
    }
    Ok(())
}

/// `arch-be model`: типизированная модель архитектуры (ADR-003).
///
/// Читающие команды (validate/show/graph) грузят модель толерантно (E3):
/// битая сущность не обнуляет весь модельный контроль — validate отчитывает
/// её error-находкой, show/graph работают по валидному подмножеству с
/// warn-пометкой. Пишущие/обменные (project/export/import) — строгие:
/// частичная модель молча потеряла бы сущности в артефактах.
fn cmd_model(cmd: ModelCmd) -> Result<()> {
    match cmd {
        ModelCmd::Validate { dir } => {
            // Аргумент принимает и корень кейса, и каталог `model/` (T-13).
            let dir = arch_harness::model::model_dir_from(&dir);
            let model = arch_harness::model::load_model_tolerant(&dir)
                .with_context(|| format!("загрузка модели {}", dir.display()))?;
            let report = arch_harness::model::validate(&model);
            for i in &report.issues {
                println!(
                    "[{}] {}: {} — {}",
                    i.severity,
                    i.file.display(),
                    i.rule,
                    i.message
                );
            }
            println!("{}", report.summary());
            println!(
                "Итог: {}",
                if report.has_errors() { "FAIL" } else { "PASS" }
            );
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        ModelCmd::Show { id, dir } => {
            let model = arch_harness::model::load_model_tolerant(&dir)
                .with_context(|| format!("загрузка модели {}", dir.display()))?;
            if let Some(note) = arch_harness::model::load_issues_note(&model.load_issues) {
                println!("Внимание: {note}");
            }
            let entity = model
                .get(&id)
                .with_context(|| format!("сущность '{id}' не найдена в {}", dir.display()))?;
            print!("{}", arch_harness::model::card(&model, entity));
        }
        ModelCmd::Graph { dir, format } => {
            let dir = arch_harness::model::model_dir_from(&dir);
            let model = arch_harness::model::load_model_tolerant(&dir)
                .with_context(|| format!("загрузка модели {}", dir.display()))?;
            if let Some(note) = arch_harness::model::load_issues_note(&model.load_issues) {
                println!("Внимание: {note}");
            }
            match format.as_str() {
                "text" => print!("{}", arch_harness::model::graph_text(&model)),
                "mermaid" => print!("{}", arch_harness::model::graph_mermaid(&model)),
                other => anyhow::bail!("неизвестный формат '{other}' (допустимы: text, mermaid)"),
            }
        }
        ModelCmd::Project { dir } => {
            let report = arch_harness::model::project_adr(&dir)
                .with_context(|| format!("проекция модели {}", dir.display()))?;
            for f in &report.written {
                println!("записан: {}", f.display());
            }
            for f in &report.removed {
                println!("удалён (нет сущности): {}", f.display());
            }
            println!(
                "Проекция {}: {} ADR-файлов, удалено устаревших: {}",
                report.out_dir.display(),
                report.written.len(),
                report.removed.len()
            );
        }
        ModelCmd::Export { dir, format } => {
            let fmt = arch_harness::model::ExportFormat::from_name(&format).with_context(|| {
                format!(
                    "неизвестный формат '{format}' (допустимы: {})",
                    arch_harness::model::ExportFormat::names()
                )
            })?;
            let model = arch_harness::model::load_model(&dir)
                .with_context(|| format!("загрузка модели {}", dir.display()))?;
            let text = arch_harness::model::export_model(&model, fmt)
                .with_context(|| format!("экспорт модели {}", dir.display()))?;
            print!("{text}");
        }
        ModelCmd::Import {
            file,
            format,
            dir,
            force,
            dry_run,
        } => {
            if format.trim().eq_ignore_ascii_case("structurizr") {
                if force || dry_run {
                    anyhow::bail!(
                        "флаги --force/--dry-run поддерживаются для форматов csv/xlsx/backstage; \
                         импорт structurizr и так не затирает файлы (коллизия — ошибка)"
                    );
                }
                let report = arch_harness::model::import_structurizr(&file, &dir)
                    .with_context(|| format!("импорт {} в {}", file.display(), dir.display()))?;
                for f in &report.written {
                    println!("записан: {}", f.display());
                }
                for w in &report.warnings {
                    println!("предупреждение: {w}");
                }
                println!(
                    "Импорт {}: {} сущностей, предупреждений: {}",
                    report.dir.display(),
                    report.written.len(),
                    report.warnings.len()
                );
                return Ok(());
            }
            let fmt =
                arch_harness::model::RegistryFormat::from_name(&format).with_context(|| {
                    format!(
                        "неизвестный формат '{format}' (допустимы: structurizr, {})",
                        arch_harness::model::RegistryFormat::names()
                    )
                })?;
            let report = arch_harness::model::import_registry(
                &file,
                &dir,
                fmt,
                &arch_harness::model::RegistryImportOptions { force, dry_run },
            )
            .with_context(|| format!("импорт {} в {}", file.display(), dir.display()))?;
            let verb = if report.dry_run {
                "записал бы"
            } else {
                "записан"
            };
            for f in &report.written {
                println!("{verb}: {}", f.display());
            }
            for s in &report.skipped {
                println!("пропущен (уже в модели): {s}");
            }
            for w in &report.warnings {
                println!("предупреждение: {w}");
            }
            println!(
                "Импорт {}: записано: {}, пропущено: {}, предупреждений: {}{}",
                report.dir.display(),
                report.written.len(),
                report.skipped.len(),
                report.warnings.len(),
                if report.dry_run { " (dry-run)" } else { "" }
            );
        }
        ModelCmd::Impact {
            dir,
            id,
            paths,
            json,
        } => {
            let report = arch_harness::review::change_impact(&dir, id.as_deref(), &paths)
                .with_context(|| format!("радиус изменения по {}", dir.display()))?;
            if json {
                let verdict = arch_harness::review::impact_json(&report);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string())
                );
            } else {
                print!("{}", arch_harness::review::render_impact(&report));
            }
        }
        ModelCmd::Drift { dir, json } => {
            let dir = arch_harness::model::case_root_from(&dir);
            let report = arch_harness::model::drift_check(&dir)
                .with_context(|| format!("дрейф «модель ↔ код» кейса {}", dir.display()))?;
            if json {
                let verdict = arch_harness::model::drift::verdict_json(&report);
                println!("{verdict:#}");
            } else {
                print!("{}", arch_harness::model::drift::render_text(&report));
            }
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        ModelCmd::Landscape {
            root,
            mermaid,
            aliases,
            diff_since,
        } => {
            let aliases = match aliases {
                Some(path) => arch_harness::landscape::load_aliases(&path)
                    .with_context(|| format!("карта алиасов {}", path.display()))?,
                None => std::collections::BTreeMap::new(),
            };
            let report = arch_harness::landscape::build_landscape_with_aliases(&root, &aliases)?;
            println!("{}", arch_harness::landscape::render_markdown(&report));
            if mermaid {
                println!("\n```mermaid");
                println!("{}", arch_harness::landscape::render_mermaid(&report));
                println!("```");
            }
            if let Some(since) = diff_since {
                let diff = arch_harness::landscape::diff_landscape(&root, &since, &aliases)
                    .with_context(|| format!("дифф ландшафта против {since}"))?;
                println!();
                println!(
                    "{}",
                    arch_harness::landscape::render_diff_markdown(&diff, &since)
                );
            }
        }
    }
    Ok(())
}

/// `arch-be trace`: трассируемость модели как fitness-функция (ADR-006).
fn cmd_trace(cfg: &Config, cmd: TraceCmd) -> Result<()> {
    match cmd {
        TraceCmd::Check { dir, format } => {
            let format = arch_harness::report_fmt::ReportFormat::parse(&format)
                .map_err(anyhow::Error::msg)?;
            // Требование исполняемой проверки инвариантов — из `[trace]` (ADR-050).
            let report = arch_harness::trace::trace_check_with(&dir, cfg.trace.executable_required)
                .with_context(|| format!("трассировка кейса {}", dir.display()))?;
            match format {
                arch_harness::report_fmt::ReportFormat::Text => {
                    print!("{}", arch_harness::trace::render_markdown(&report));
                }
                machine => {
                    print!(
                        "{}",
                        arch_harness::report_fmt::render(
                            machine,
                            &arch_harness::report_fmt::FmtReport::from_trace(&report),
                        )
                    );
                }
            }
            if report.has_errors() {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be nfr`: количественные NFR поверх модели (ADR-007); error — exit code 1.
fn cmd_nfr(cmd: NfrCmd) -> Result<()> {
    match cmd {
        NfrCmd::Budget { dir } => {
            let report = arch_harness::nfr::budget_check(&dir)
                .with_context(|| format!("latency-бюджет кейса {}", dir.display()))?;
            print!("{}", report.render());
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        NfrCmd::Availability { dir } => {
            let report = arch_harness::nfr::availability_check(&dir)
                .with_context(|| format!("расчёт доступности кейса {}", dir.display()))?;
            print!("{}", report.render());
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        NfrCmd::Capacity { dir } => {
            let report = arch_harness::nfr::capacity_check(&dir)
                .with_context(|| format!("расчёт ёмкости кейса {}", dir.display()))?;
            print!("{}", report.render());
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        NfrCmd::Cost { dir } => {
            let report = arch_harness::nfr::cost_check(&dir)
                .with_context(|| format!("расчёт стоимости кейса {}", dir.display()))?;
            print!("{}", report.render());
            if report.has_errors() {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be skills`: библиотека скиллов.
fn cmd_skills(cfg: &Config, cmd: SkillsCmd) -> Result<()> {
    // T-09: индекс — настроенные плагины ПЛЮС скиллы, разложенные в проекте
    // (`connect`); без второго поиск пуст в свежем проекте.
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dirs = arch_harness::plugin::skill_search_dirs(&cfg.plugins.dirs, &root);
    let plugins = arch_harness::plugin::discover(&dirs);
    match cmd {
        SkillsCmd::List => {
            let total: usize = plugins.iter().map(|p| p.skills.len()).sum();
            println!("Скиллов: {total} в {} плагинах", plugins.len());
            for p in &plugins {
                for s in &p.skills {
                    println!(
                        "  {:<28} {:<14} {}",
                        s.name,
                        p.manifest.name,
                        first_line(&s.description, 80)
                    );
                }
            }
        }
        SkillsCmd::Search { query, limit } => {
            let hits = arch_harness::plugin::search(&plugins, &query, limit);
            if hits.is_empty() {
                println!(
                    "{}.",
                    arch_harness::plugin::empty_index_answer(
                        &query,
                        plugins.iter().map(|p| p.skills.len()).sum::<usize>(),
                        &dirs
                    )
                );
            }
            for h in &hits {
                println!(
                    "── {} [{}] (score {:.1})",
                    h.meta.name, h.meta.plugin, h.score
                );
                println!("   {}", first_line(&h.meta.description, 100));
                if !h.snippet.is_empty() {
                    println!("{}", h.snippet);
                }
            }
        }
        SkillsCmd::Show { name } => {
            let meta = arch_harness::plugin::skill_by_name(&plugins, &name)
                .with_context(|| format!("скилл '{name}' не найден (см. `arch-be skills list`)"))?;
            println!("{}", arch_harness::plugin::load_skill(meta)?);
        }
    }
    Ok(())
}

/// `arch-be plugins`: пакеты скиллов + MCP.
fn cmd_plugins(cfg: &Config, cmd: PluginsCmd) -> Result<()> {
    let plugins = arch_harness::plugin::discover(&cfg.plugins.dirs);
    match cmd {
        PluginsCmd::List => {
            println!("Плагины ({}):", plugins.len());
            for p in &plugins {
                let mcp_count = if cfg.plugins.include_mcp {
                    arch_harness::plugin::mcp_servers(std::slice::from_ref(p)).len()
                } else {
                    0
                };
                println!(
                    "  {:<24} v{:<8} скиллов: {:<3} mcp: {:<2} {}",
                    p.manifest.name,
                    p.manifest.version,
                    p.skills.len(),
                    mcp_count,
                    first_line(&p.manifest.description, 60)
                );
            }
        }
        PluginsCmd::Show { name } => {
            let p = plugins
                .iter()
                .find(|p| p.manifest.name == name)
                .with_context(|| format!("плагин '{name}' не найден"))?;
            println!(
                "{} v{} — {}",
                p.manifest.name, p.manifest.version, p.manifest.description
            );
            println!("Каталог: {}", p.dir.display());
            if !p.manifest.keywords.is_empty() {
                println!("Ключевые слова: {}", p.manifest.keywords.join(", "));
            }
            println!("Скиллы ({}):", p.skills.len());
            for s in &p.skills {
                println!("  {:<28} {}", s.name, first_line(&s.description, 70));
            }
            let servers = arch_harness::plugin::mcp_servers(std::slice::from_ref(p));
            if !servers.is_empty() {
                println!("MCP-серверы ({}):", servers.len());
                for s in &servers {
                    println!("  {:<24} {} {}", s.name, s.command, s.args.join(" "));
                }
            }
        }
    }
    Ok(())
}

/// Первая строка текста, усечённая до `max` символов.
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let cut: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        format!("{cut}…")
    } else {
        cut
    }
}

/// `arch-be policy`: уровень автономии и классификация команды.
fn cmd_policy(cfg: &Config, check: Option<String>) -> Result<()> {
    let policy = arch_harness::policy::Policy::parse(&cfg.policy.autonomy)?;
    match check {
        None => {
            println!(
                "Уровень автономии: R{} (из config [policy] autonomy)",
                policy.level
            );
            println!(
                "  R0 — только чтения авто; R2 — + изменения (дефолт); R4 — деструктив с подтверждением; R5 — полная (красный флаг аудита)"
            );
        }
        Some(cmd) => {
            use arch_harness::policy::{PolicyDecision, classify_bash};
            let class = classify_bash(&cmd);
            let decision = policy.check("bash", &serde_json::json!({"command": cmd}));
            let verdict = match &decision {
                PolicyDecision::Allow => "ALLOW",
                PolicyDecision::RequireConfirm(_) => "REQUIRE-CONFIRM",
                PolicyDecision::Deny(_) => "DENY",
            };
            println!(
                "команда: {cmd}\nкласс риска: {class:?}\nрешение (R{}): {verdict}",
                policy.level
            );
            match &decision {
                PolicyDecision::RequireConfirm(m) | PolicyDecision::Deny(m) => {
                    println!("причина: {m}");
                }
                PolicyDecision::Allow => {}
            }
        }
    }
    Ok(())
}

/// `arch-be evidence`: Evidence Bundle.
fn cmd_evidence(cfg: &arch_harness::config::Config, cmd: EvidenceCmd) -> Result<()> {
    match cmd {
        EvidenceCmd::Pack { dir, route } => {
            let route = match route.to_lowercase().as_str() {
                "fast" => arch_harness::control::Route::Fast,
                "critical" => arch_harness::control::Route::Critical,
                _ => arch_harness::control::Route::Standard,
            };
            let (bundle, verdict) = arch_harness::evidence::pack(&dir, route)?;
            println!("{}", verdict.summary);
            for item in &bundle.items {
                println!("  + {:<20} {} ({} б)", item.key, item.path, item.size);
            }
            for miss in &verdict.missing {
                println!("  ✗ ОТСУТСТВУЕТ: {miss}");
            }
            println!("Манифест: {}", dir.join("EVIDENCE.yaml").display());
            if !verdict.passed {
                std::process::exit(1);
            }
        }
        EvidenceCmd::Verify { dir } => {
            let v = arch_harness::evidence::verify_with(&dir, &cfg.evidence)?;
            println!("{}", v.summary);
            for w in &v.warnings {
                println!("  ⚠ {w}");
            }
            for m in &v.missing {
                println!("  ✗ ОТСУТСТВУЕТ: {m}");
            }
            for t in &v.tampered {
                println!("  ✗ ИЗМЕНЁН: {t}");
            }
            // Содержание артефактов (Н1, ADR-041): «есть» ≠ «написан».
            for f in &v.semantics {
                let mark = if f.severity == "error" { "✗" } else { "⚠" };
                println!("  {mark} [{}] {}: {}", f.rule, f.key, f.message);
                println!("      → {}", f.fix_hint);
            }
            // Заявленное, но механикой не проверяемое — печатается всегда:
            // Spine не притворяется, что удостоверил подпись или смысл.
            for n in &v.not_verified {
                println!("  · не проверяется механикой: {n}");
            }
            println!(
                "Итог: {}",
                if v.passed {
                    "PASS — выпуск разрешён"
                } else {
                    "FAIL — выпуск заблокирован"
                }
            );
            // W1: вердикт бандла — часть вердикта гейта, а тот печатает
            // паспорт. Ссылка нужна здесь, потому что читатель бандла до
            // гейта может и не дойти.
            println!(
                "Паспорт вердикта (что зелёный НЕ означает): {}",
                arch_harness::passport::hint_command(&dir)
            );
            if !v.passed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be delta`: дельта-спецификации.
fn cmd_delta(cmd: DeltaCmd) -> Result<()> {
    let cwd = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match cmd {
        DeltaCmd::New { name, repo } => {
            let path = arch_harness::delta::new(&repo.unwrap_or_else(cwd), &name)?;
            println!("Дельта создана: {}", path.display());
        }
        DeltaCmd::List { repo } => {
            let list = arch_harness::delta::list(&repo.unwrap_or_else(cwd));
            if list.is_empty() {
                println!("Дельт нет (changes/ пуст или отсутствует).");
            }
            for d in &list {
                println!("  {:<30} {:?}", d.name, d.status);
            }
        }
        DeltaCmd::Validate { name, repo } => {
            let issues = arch_harness::delta::validate(&repo.unwrap_or_else(cwd), &name)?;
            if issues.is_empty() {
                println!("дельта '{name}': нарушений нет");
            }
            let mut failed = false;
            for i in &issues {
                println!(
                    "[{}] {}:{} {} — {}",
                    i.severity,
                    i.file.display(),
                    i.line,
                    i.rule,
                    i.message
                );
                failed |= i.severity == "error";
            }
            if failed {
                std::process::exit(1);
            }
        }
        DeltaCmd::Archive { name, repo } => {
            let root = repo.unwrap_or_else(cwd);
            let path = arch_harness::delta::archive(&root, &name)?;
            println!("Дельта заархивирована: {}", path.display());
            if let Some(hint) = arch_harness::delta::archive_order_hint(&root) {
                println!("  → {hint}");
            }
        }
        DeltaCmd::Guard {
            repo,
            base,
            protect,
        } => {
            let report =
                arch_harness::delta::guard(&repo.unwrap_or_else(cwd), base.as_deref(), &protect)?;
            print!("{}", arch_harness::delta::render_guard(&report));
            if !report.passed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be openspec`: адаптер `OpenSpec` — требования → покрытие fitness-правилами.
fn cmd_openspec(cmd: OpenspecCmd) -> Result<()> {
    match cmd {
        OpenspecCmd::Scan { root, json } => {
            let requirements = arch_harness::openspec::scan_requirements(&root)?;
            if json {
                let report = arch_harness::openspec::ScanReport {
                    total: requirements.len(),
                    root,
                    requirements,
                };
                // SDK-контракт v1: машиночитаемый отчёт в stdout.
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("ScanReport сериализуется")
                );
            } else {
                print!(
                    "{}",
                    arch_harness::openspec::render_scan(&root, &requirements)
                );
            }
        }
        OpenspecCmd::Coverage {
            root,
            constraints,
            json,
            strict,
        } => {
            let report = arch_harness::openspec::coverage(&root, constraints.as_deref())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("CoverageReport сериализуется")
                );
            } else {
                print!("{}", report.to_markdown());
            }
            // Строгий режим: требования «без решения» ломают гейт (exit 1).
            if strict && report.unresolved > 0 {
                std::process::exit(1);
            }
        }
        OpenspecCmd::Init { root, out, force } => {
            let out_dir = out.unwrap_or_else(|| root.clone());
            let outcome = arch_harness::openspec::init(&root, &out_dir, force)?;
            println!("Записано: {}", outcome.constraints_path.display());
            println!("Записано: {}", outcome.spine_path.display());
            println!(
                "Правил-заглушек: {}, кандидатов в спайн: {}, истории (archive): {}",
                outcome.rules, outcome.candidates, outcome.history
            );
            print!("{}", outcome.coverage.to_markdown());
        }
        OpenspecCmd::Gate {
            archive,
            root,
            change_id,
            constraints,
        } => {
            if !archive {
                anyhow::bail!(
                    "реализован только гейт --archive (roadmap: --change, --expiry — docs/openspec.md)"
                );
            }
            let report =
                arch_harness::openspec::gate_archive(&root, &change_id, constraints.as_deref())?;
            print!("{}", report.to_markdown());
            if !report.passed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be agents-md`: AGENTS.md как канал архитектурного контроля.
fn cmd_agents_md(cfg: &Config, cmd: AgentsMdCmd) -> Result<()> {
    match cmd {
        AgentsMdCmd::Refresh { repo } => {
            let report = arch_harness::agentsmd::generate(&repo)?;
            println!(
                "AGENTS.md: {} ({}) — инвариантов: {}, fitness: {}",
                report.path.display(),
                report.action,
                report.invariants,
                if report.has_constraints {
                    "да"
                } else {
                    "нет"
                }
            );
        }
        AgentsMdCmd::Lint { repo } => {
            let issues = arch_harness::agentsmd::lint(&repo)?;
            if issues.is_empty() {
                println!("AGENTS.md свежий, нарушений нет");
            }
            let mut failed = false;
            for i in &issues {
                println!(
                    "[{}] {}:{} {} — {}",
                    i.severity,
                    i.file.display(),
                    i.line,
                    i.rule,
                    i.message
                );
                failed |= i.severity == "error";
            }
            if failed {
                std::process::exit(1);
            }
        }
        AgentsMdCmd::LintAll { registry } => {
            let registry = registry.unwrap_or_else(|| Config::home_dir().join("repos.txt"));
            let results = arch_harness::agentsmd::lint_registry(&registry)?;
            let mut failed = false;
            for (repo, issues) in &results {
                let errors = issues.iter().filter(|i| i.severity == "error").count();
                let status = if issues.is_empty() {
                    "OK".to_string()
                } else {
                    format!("{} проблем ({} error)", issues.len(), errors)
                };
                println!("{:<50} {}", repo.display(), status);
                failed |= errors > 0;
            }
            let _ = cfg;
            if failed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be cron` (только сборка `harness`).
#[cfg(feature = "harness")]
async fn cmd_cron(cfg: &Arc<Config>, cmd: CronCmd) -> Result<()> {
    let tab = arch_harness::cron::load(&cfg.cron.file)?;
    match cmd {
        CronCmd::List => {
            for j in &tab.jobs {
                println!(
                    "  {:<24} {:<16} {}",
                    j.name,
                    j.schedule,
                    j.task_md.display()
                );
            }
        }
        CronCmd::Run { name } => {
            let job = tab
                .jobs
                .iter()
                .find(|j| j.name == name)
                .with_context(|| format!("задача '{name}' не найдена"))?;
            let registry = LlmRegistry::from_config(cfg)?;
            let provider = match &job.model {
                Some(m) => registry.get(m)?,
                None => registry.default(),
            };
            let tools = arch_harness::tools::full_registry(cfg);
            let out_dir = job
                .out
                .clone()
                .unwrap_or_else(|| cfg.paths.reports_dir.join("cron"));
            let path =
                arch_harness::cron::run_job(job, provider.as_ref(), &tools, &out_dir).await?;
            println!("Отчёт: {}", path.display());
        }
        CronCmd::Tick => {
            // «Дюжные» задачи между прошлым тиком и сейчас; метка — в state-файле.
            let state_file = Config::home_dir().join("cron-last-tick");
            let now = chrono::Local::now();
            let last = std::fs::read_to_string(&state_file)
                .ok()
                .and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(s.trim())
                        .ok()
                        .map(|dt| dt.with_timezone(&chrono::Local))
                })
                .unwrap_or_else(|| now - chrono::Duration::hours(24));
            let registry = LlmRegistry::from_config(cfg)?;
            let provider = registry.default();
            let tools = arch_harness::tools::full_registry(cfg);
            let reports_dir = cfg
                .cron
                .out_dir
                .clone()
                .unwrap_or_else(|| cfg.paths.reports_dir.join("cron"));
            let reports = arch_harness::cron::run_due(
                &tab,
                last,
                now,
                provider.as_ref(),
                &tools,
                &reports_dir,
            )
            .await?;
            std::fs::write(&state_file, now.to_rfc3339()).context("запись метки тика")?;
            if reports.is_empty() {
                println!("Дюжных задач нет.");
            }
            for path in &reports {
                println!("Отчёт: {}", path.display());
            }
        }
    }
    Ok(())
}

/// `arch eval`: регрессионные eval-сьюты конфигурации харнесса (docs/evals.md).
///
/// Встроенный сьют (без `--suite`) герметичен: ассеты и конфиг разворачиваются
/// во временный каталог, живой `~/.arch-harness` не трогается — прогон зелёный
/// и в CI без `arch init`. Пользовательский `--suite` бежит против живой
/// установки. Гейт: pass-rate ниже `--gate` (дефолт 100%) — exit code 1.
#[cfg(feature = "harness")]
async fn cmd_eval(cfg: &Arc<Config>, cmd: EvalCmd) -> Result<()> {
    match cmd {
        EvalCmd::Run {
            suite,
            gate,
            judge,
            model,
        } => {
            let gate_pct = gate.unwrap_or(100.0);
            if !(0.0..=100.0).contains(&gate_pct) {
                anyhow::bail!("--gate: ожидается процент 0..=100, получено {gate_pct}");
            }
            // tempdir держим живым до конца прогона (встроенный сьют).
            let mut _tmp = None;
            let (suite_dir, ctx) = if let Some(dir) = &suite {
                (dir.clone(), arch_harness::eval::SuiteContext::for_live()?)
            } else {
                let tmp = tempfile::tempdir().context("временный каталог встроенного сьюта")?;
                let home = arch_harness::eval::prepare_builtin_home(tmp.path())?;
                let ctx = arch_harness::eval::SuiteContext::for_builtin(tmp.path(), &home.config)?;
                _tmp = Some(tmp);
                (home.suite_dir, ctx)
            };
            // Слой судьи: реестр моделей строится только при --judge —
            // офлайн-прогон не требует ни ключей, ни сети.
            let provider = if judge {
                let registry = LlmRegistry::from_config(cfg)?;
                Some(match &model {
                    Some(name) => registry.get(name)?,
                    None => registry.default(),
                })
            } else {
                None
            };
            let rubrics_dir = cfg.paths.rubrics_dir();
            let judge_ctx = provider.as_ref().map(|p| arch_harness::eval::JudgeCtx {
                provider: p.as_ref(),
                cfg: &cfg.judge,
                rubrics_dir: &rubrics_dir,
            });
            let report =
                arch_harness::eval::run_suite(&suite_dir, &ctx, judge_ctx.as_ref(), gate_pct)
                    .await?;
            print!("{}", arch_harness::eval::render_text(&report));
            let out = arch_harness::eval::write_report(&report, &cfg.paths.evals_dir())?;
            eprintln!("Отчёт: {}", out.display());
            if !report.gate_passed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
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
fn resolve_asset(dir: &std::path::Path, name: &str, ext: &str) -> PathBuf {
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

/// Метка времени для имён отчётов.
fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
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
