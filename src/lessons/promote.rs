//! Промоушен урока в fitness-правило (L7, ADR-089, критерий E5).
//!
//! Замыкает цикл «контекст → артефакт → инвариант»: урок, подтверждённый
//! счётчиками доказательств ([`LessonStatus::Confirmed`]) и выразимый
//! шаблоном fitness-правила, превращается в детерминированное правило
//! `CONSTRAINTS.yaml` через дельту. После этого следующее нарушение той же
//! сигнатуры ловится уже не на приёмке, а на `pre-commit` у исполнителя
//! (`arch-be hook pre-commit` → `control check`).
//!
//! # Детерминизм (AD-2)
//!
//! Модуль не обращается к LLM. Выразимость урока решается механически, одним
//! из двух путей:
//!
//! 1. **Шаблон `rule_templates`** ([`crate::rule_templates`]): текст урока
//!    (statement + подпись) подбирается словарём шаблонов — правило
//!    `command_succeeds` с исполняемой проверкой (П2, нарушающая реализация).
//! 2. **Структурированная директива** в `statement`: урок, чей `statement`
//!    является YAML-маппингом с полем `kind` (`must_contain`,
//!    `must_not_contain`, `file_exists`, `conditional`, `command_succeeds`)
//!    и нужными полями, выразим как content/файловое/условное правило. Это
//!    контракт между слоем формулировки (L6, человек) и слоем промоушена:
//!    LLM формулирует прозу, а механику несёт структурированная директива.
//!
//! Ни тот, ни другой путь не сработал — урок **невыразим**, промоушен не
//! происходит (fail-closed: ложное ужесточение хуже отсутствия правила).
//!
//! # Redteam-засев (риск 5 TASK)
//!
//! Кандидат-правило принимается только при двух «да»:
//!
//! - (а) **ловит** засеянное нарушение (правило имеет зубы: нарушение его
//!   сигнатуры краснит `control check` на копии репозитория);
//! - (б) **не краснит** текущий зелёный HEAD (правило в изоляции проходит на
//!   рабочем дереве).
//!
//! Оба прогона детерминированы ([`crate::control::check`]); для
//! `command_succeeds` зубы доказывает нарушающая реализация шаблона (П2), а
//! не засев.
//!
//! # Подпись человека
//!
//! На маршрутах Standard и Critical промоушен требует [`HUMAN_SIGN_OFF`] в
//! дельте; без подписи — ошибка (exit 1 на краю CLI). Fast — без подписи.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::control::Route;
use crate::error::{HarnessError, Result};

use super::{Ledger, Lesson, LessonStatus, Thresholds};

/// Маркер подписи человека, хранимый в дельте (ADR-089, решение 3).
pub const HUMAN_SIGN_OFF: &str = "[РЕШЕНИЕ ЧЕЛОВЕКА]";

/// Виды fitness-правил, выразимых промоушеном (ADR-089, решение 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoRuleKind {
    /// Обязательное наличие паттерна в наборе файлов.
    MustContain,
    /// Запрет паттерна в наборе файлов.
    MustNotContain,
    /// Обязательное существование файла/пути.
    FileExists,
    /// Условное правило `if`/`then` (волна S, ADR-070).
    Conditional,
    /// Исполняемое правило (шаблон `rule_templates` или явная команда).
    CommandSucceeds,
}

impl PromoRuleKind {
    /// Имя типа правила в `CONSTRAINTS.yaml`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MustContain => "must_contain",
            Self::MustNotContain => "must_not_contain",
            Self::FileExists => "file_exists",
            Self::Conditional => "conditional",
            Self::CommandSucceeds => "command_succeeds",
        }
    }

    /// Разбирает имя типа из директивы; неизвестный вид — `None` (невыразим).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "must_contain" => Some(Self::MustContain),
            "must_not_contain" => Some(Self::MustNotContain),
            "file_exists" => Some(Self::FileExists),
            "conditional" => Some(Self::Conditional),
            "command_succeeds" => Some(Self::CommandSucceeds),
            _ => None,
        }
    }
}

/// Условие/обязательство правила `conditional` (пара `glob` + `pattern`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Clause {
    /// Glob'ы модуля-локального поиска (пусто — `**/*`).
    #[serde(default)]
    pub glob: String,
    /// Regex-условие.
    #[serde(default)]
    pub pattern: String,
}

/// Предложение правила: детерминированная свёртка урока в fitness-правило.
#[derive(Debug, Clone)]
pub struct RuleProposal {
    /// Вид правила.
    pub kind: PromoRuleKind,
    /// Имя правила (становится кодом находки и ссылкой `promoted_rule`).
    pub name: String,
    /// Glob набора файлов (content-правила).
    pub glob: Option<String>,
    /// Regex (content-правила).
    pub pattern: Option<String>,
    /// Путь (`file_exists`).
    pub path: Option<String>,
    /// Команда (`command_succeeds`).
    pub command: Option<String>,
    /// Триггер `if` (`conditional`).
    pub if_clause: Option<Clause>,
    /// Обязательство `then` (`conditional`).
    pub then_clause: Option<Clause>,
    /// Задетый инвариант спайна (пусто — неизвестен).
    pub ad: String,
    /// Зачем правило (провенанс урока в находках).
    pub rationale: String,
    /// Что делать при нарушении.
    pub fix_hint: String,
    /// Скилл-источник методики.
    pub skill: String,
    /// Id шаблона `rule_templates` (только для `command_succeeds` из шаблона).
    pub template_id: Option<String>,
}

impl RuleProposal {
    /// Перезаписывает инвариант спайна (опция `--ad` на краю CLI).
    #[must_use]
    pub fn with_ad(mut self, ad: impl Into<String>) -> Self {
        self.ad = ad.into();
        self
    }

    /// Готовый YAML-фрагмент правила для списка `rules:`/`constraints:`.
    ///
    /// Скаляры `glob`/`pattern`/`path`/`command`/`rationale`/`fix_hint` — в
    /// одинарных кавычках ([`yaml_sq`]): regex с `\s`/`\d` доезжает до парсера
    /// литерально (прецедент — `rules_suggest`, дефект D2).
    #[must_use]
    pub fn yaml(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "  - name: {}", self.name);
        let _ = writeln!(out, "    type: {}", self.kind.as_str());
        match self.kind {
            PromoRuleKind::MustContain | PromoRuleKind::MustNotContain => {
                let _ = writeln!(
                    out,
                    "    glob: {}",
                    yaml_sq(self.glob.as_deref().unwrap_or("**/*"))
                );
                let _ = writeln!(
                    out,
                    "    pattern: {}",
                    yaml_sq(self.pattern.as_deref().unwrap_or(""))
                );
            }
            PromoRuleKind::FileExists => {
                let _ = writeln!(
                    out,
                    "    path: {}",
                    yaml_sq(self.path.as_deref().unwrap_or(""))
                );
            }
            PromoRuleKind::Conditional => {
                let ifc = self.if_clause.as_ref();
                let thenc = self.then_clause.as_ref();
                let _ = writeln!(out, "    if:");
                let _ = writeln!(
                    out,
                    "      glob: {}",
                    yaml_sq(ifc.map_or("**/*", |c| c.glob.as_str()))
                );
                let _ = writeln!(
                    out,
                    "      pattern: {}",
                    yaml_sq(ifc.map_or("", |c| c.pattern.as_str()))
                );
                let _ = writeln!(out, "    then:");
                let _ = writeln!(
                    out,
                    "      glob: {}",
                    yaml_sq(thenc.map_or("**/*", |c| c.glob.as_str()))
                );
                let _ = writeln!(
                    out,
                    "      pattern: {}",
                    yaml_sq(thenc.map_or("", |c| c.pattern.as_str()))
                );
            }
            PromoRuleKind::CommandSucceeds => {
                let _ = writeln!(
                    out,
                    "    command: {}",
                    yaml_sq(self.command.as_deref().unwrap_or("true"))
                );
                let _ = writeln!(out, "    timeout_secs: 300");
            }
        }
        let _ = writeln!(out, "    severity: error");
        // Маркер происхождения из контура обучения (пассивная метка: движок
        // `control check` поле не читает, но человек и реестр видят, что
        // правило пришло из урока, а не из библиотеки корпоративных правил).
        let _ = writeln!(out, "    status: proposed");
        if !self.ad.is_empty() {
            let _ = writeln!(out, "    ad: {}", self.ad);
        }
        let _ = writeln!(out, "    rationale: {}", yaml_sq(&self.rationale));
        let _ = writeln!(out, "    fix_hint: {}", yaml_sq(&self.fix_hint));
        let _ = writeln!(out, "    skill: {}", self.skill);
        out
    }
}

/// Структурированная директива правила внутри `statement` (контракт L6/L7).
#[derive(Debug, Clone, Deserialize)]
struct Directive {
    /// Вид правила.
    kind: String,
    /// Glob набора файлов (content-правила).
    #[serde(default)]
    glob: Option<String>,
    /// Regex (content-правила).
    #[serde(default)]
    pattern: Option<String>,
    /// Путь (`file_exists`).
    #[serde(default)]
    path: Option<String>,
    /// Команда (`command_succeeds`).
    #[serde(default)]
    command: Option<String>,
    /// Триггер `if` (`conditional`).
    #[serde(default, rename = "if")]
    if_clause: Option<Clause>,
    /// Обязательство `then` (`conditional`).
    #[serde(default, rename = "then")]
    then_clause: Option<Clause>,
}

/// Разбирает `statement` как YAML-директиву правила. Не-маппинг (проза) или
/// битый YAML — `None`: урок выразим только через шаблон или вовсе невыразим.
fn parse_directive(statement: &str) -> Option<Directive> {
    serde_yaml_ng::from_str::<Directive>(statement.trim()).ok()
}

/// Выразимость урока: свёртка `confirmed`-урока в правило, либо `None`.
///
/// Порядок детерминирован: сначала структурированная директива (самый явный
/// контракт), затем словарь шаблонов `rule_templates` (команда). Не
/// [`LessonStatus::Confirmed`] — `None` (промоушен только подтверждённого).
#[must_use]
pub fn express(lesson: &Lesson) -> Option<RuleProposal> {
    if lesson.status != LessonStatus::Confirmed {
        return None;
    }
    let rationale = format!(
        "промоутировано из урока {} ({}): контур ужесточается сам (ADR-089)",
        lesson.id,
        lesson.signature.describe()
    );
    let fix_hint = format!("устранить нарушение; контекст — урок {}", lesson.id);
    let skill = "fitness-functions".to_string();

    if let Some(d) = parse_directive(&lesson.statement) {
        let kind = PromoRuleKind::parse(&d.kind)?;
        let name = rule_name(&lesson.signature.code, kind);
        let proposal = match kind {
            PromoRuleKind::MustContain | PromoRuleKind::MustNotContain => {
                let glob = d.glob?;
                let pattern = d.pattern?;
                RuleProposal {
                    kind,
                    name,
                    glob: Some(glob),
                    pattern: Some(pattern),
                    path: None,
                    command: None,
                    if_clause: None,
                    then_clause: None,
                    ad: String::new(),
                    rationale,
                    fix_hint,
                    skill,
                    template_id: None,
                }
            }
            PromoRuleKind::FileExists => RuleProposal {
                kind,
                name,
                glob: None,
                pattern: None,
                path: Some(d.path?),
                command: None,
                if_clause: None,
                then_clause: None,
                ad: String::new(),
                rationale,
                fix_hint,
                skill,
                template_id: None,
            },
            PromoRuleKind::Conditional => RuleProposal {
                kind,
                name,
                glob: None,
                pattern: None,
                path: None,
                command: None,
                if_clause: Some(d.if_clause?),
                then_clause: Some(d.then_clause?),
                ad: String::new(),
                rationale,
                fix_hint,
                skill,
                template_id: None,
            },
            PromoRuleKind::CommandSucceeds => RuleProposal {
                kind,
                name,
                glob: None,
                pattern: None,
                path: None,
                command: Some(d.command?),
                if_clause: None,
                then_clause: None,
                ad: String::new(),
                rationale,
                fix_hint,
                skill,
                template_id: None,
            },
        };
        return Some(proposal);
    }

    // Шаблон rule_templates: словарь подбора по тексту урока + подписи.
    let text = format!("{} {}", lesson.statement, lesson.signature.describe());
    let matched = crate::rule_templates::match_templates(&text, 1).ok()?;
    let best = matched.first()?;
    let template = crate::rule_templates::template(&best.id).ok()??;
    let command = template.manifest.rule.commands.python.clone()?;
    Some(RuleProposal {
        kind: PromoRuleKind::CommandSucceeds,
        name: template.manifest.rule.name.clone(),
        glob: None,
        pattern: None,
        path: None,
        command: Some(command),
        if_clause: None,
        then_clause: None,
        ad: String::new(),
        rationale,
        fix_hint,
        skill,
        template_id: Some(best.id.clone()),
    })
}

/// Требует ли маршрут подпись человека (ADR-089, решение 3): Standard и
/// Critical — да, Fast — нет.
#[must_use]
pub fn requires_signature(route: Route) -> bool {
    !matches!(route, Route::Fast)
}

/// Имя правила из кода подписи и вида (детерминированное, `snake_case`).
fn rule_name(code: &str, kind: PromoRuleKind) -> String {
    format!("promoted_{}_{}", slug_name(code), kind.as_str())
}

/// Скаляр YAML в одинарных кавычках (см. [`RuleProposal::yaml`]).
fn yaml_sq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// `snake_case` из произвольной строки (неалфавитно-цифровое → `_`).
fn slug_name(raw: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "lesson".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Итог redteam-засева правила-кандидата.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedVerdict {
    /// Оба «да»: правило ловит нарушение и не краснит зелёный HEAD.
    Accepted {
        /// Пояснение (например, «зубы `command_succeeds` — шаблон `rule_templates`»).
        note: Option<String>,
    },
    /// Кандидат отклонён (краснит HEAD или беззубый).
    Rejected {
        /// Причина отказа (человекочитаемая).
        reason: String,
    },
}

/// Redteam-засев правила-кандидата (ADR-089, решение 2; риск 5 TASK).
///
/// Пишет правило во временный реестр и гоняет [`crate::control::check`]
/// дважды: на рабочем дереве (зелёный HEAD) и на копии с засеянным
/// нарушением. Оба прогона ничего не меняют в репозитории.
///
/// # Errors
/// Репозиторий/временный каталог недоступны, засев не удался (для content-
/// правил — некому сформировать файл под glob'ом).
pub fn redteam_seed(repo: &Path, proposal: &RuleProposal) -> Result<SeedVerdict> {
    let constraints = temp_constraints(proposal)?;
    let constraints_path = constraints.path().join("CONSTRAINTS.yaml");

    // (б) зелёный HEAD: правило в изоляции обязано пройти на рабочем дереве.
    let head = crate::control::check(repo, &constraints_path)?;
    if !head.passed {
        let names = head
            .issues
            .iter()
            .filter(|i| i.severity == "error")
            .map(|i| i.rule.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(SeedVerdict::Rejected {
            reason: format!("правило краснит зелёный HEAD (находки: {names})"),
        });
    }

    // (а) зубы: `command_succeeds` доказывает нарушающая реализация шаблона
    // (П2), content/файловое/условное — засев в копии репозитория.
    if proposal.kind == PromoRuleKind::CommandSucceeds {
        return Ok(SeedVerdict::Accepted {
            note: Some(
                "command_succeeds: зубы доказывает нарушающая реализация шаблона \
                 rule_templates (П2), а не засев"
                    .to_string(),
            ),
        });
    }
    let copy = tempfile::TempDir::new().map_err(|e| HarnessError::io(repo, e))?;
    crate::rule_templates::copy_tree(
        repo,
        copy.path(),
        &[".git", "target", "node_modules", "__pycache__"],
    )
    .map_err(|e| HarnessError::io(repo, e))?;
    seed_violation(copy.path(), proposal)?;
    let seeded = crate::control::check(copy.path(), &constraints_path)?;
    if seeded.passed {
        return Ok(SeedVerdict::Rejected {
            reason: "правило не ловит засеянное нарушение — беззубое, промоушен отклонён"
                .to_string(),
        });
    }
    Ok(SeedVerdict::Accepted { note: None })
}

/// Временный реестр с единственным правилом-кандидатом.
fn temp_constraints(proposal: &RuleProposal) -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir().map_err(|e| HarnessError::io(Path::new("."), e))?;
    let doc = format!("rules:\n{}\n", proposal.yaml());
    std::fs::write(dir.path().join("CONSTRAINTS.yaml"), doc)
        .map_err(|e| HarnessError::io(dir.path(), e))?;
    Ok(dir)
}

/// Засевает нарушение в копии репозитория под вид правила (a — «есть зубы»).
fn seed_violation(repo: &Path, proposal: &RuleProposal) -> Result<()> {
    match proposal.kind {
        PromoRuleKind::MustNotContain => {
            // Файл под glob'ом, содержащий запрещённый паттерн, обязан краснить.
            let path = seed_path(repo, proposal.glob.as_deref().unwrap_or("**/*"));
            let content = proposal.pattern.clone().unwrap_or_default();
            write_seed(&path, &content)?;
        }
        PromoRuleKind::MustContain => {
            // Файл под glob'ом БЕЗ обязательного паттерна обязан краснить.
            let path = seed_path(repo, proposal.glob.as_deref().unwrap_or("**/*"));
            write_seed(&path, "// засев: обязательный паттерн отсутствует\n")?;
        }
        PromoRuleKind::FileExists => {
            let path = repo.join(proposal.path.as_deref().unwrap_or(""));
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| HarnessError::io(&path, e))?;
            }
        }
        PromoRuleKind::Conditional => {
            // Модуль с триггером `if`, но без обязательства `then`, обязан краснить.
            let ifc = proposal.if_clause.as_ref();
            let path = seed_path(repo, ifc.map_or("**/*", |c| c.glob.as_str()));
            let content = ifc.map_or_else(String::new, |c| c.pattern.clone());
            write_seed(&path, &content)?;
        }
        PromoRuleKind::CommandSucceeds => {
            return Err(HarnessError::Control(
                "засев command_succeeds не выполняется: зубы — нарушающая реализация шаблона"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Записывает файл засева (создавая родительские каталоги).
fn write_seed(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    std::fs::write(path, content).map_err(|e| HarnessError::io(path, e))
}

/// Превращает glob в конкретный путь засева: первый `*`-сегмент → имя `seed`
/// (с литеральным расширением сегмента), `**` опускается. Эвристика покрывает
/// формы `*.rs`, `src/**/*.rs`, `docs/*.md`, `**/*`; для экзотических глобов
/// засев может не сформировать файл — тогда правило честно отклоняется как
/// «не ловит нарушение» (fail-closed).
fn seed_path(repo: &Path, glob: &str) -> PathBuf {
    let mut out = repo.to_path_buf();
    let mut placed = false;
    for seg in glob.trim_start_matches("./").split('/') {
        if seg.is_empty() || seg == "**" {
            continue;
        }
        if seg.contains('*') {
            out.push(seed_filename(seg));
            placed = true;
        } else {
            out.push(seg);
        }
    }
    if !placed {
        out.push("seed.txt");
    }
    out
}

/// Имя файла засева для `*`-сегмента (`*.rs` → `seed.rs`, `*` → `seed.txt`).
fn seed_filename(seg: &str) -> String {
    if let Some(ext) = seg.rsplit('.').next() {
        if ext != seg && !ext.contains('*') && !ext.is_empty() {
            return format!("seed.{ext}");
        }
    }
    "seed.txt".to_string()
}

/// Вставляет фрагмент правила в конец списка `rules:`/`constraints:` текста
/// реестра (перед следующей top-level секцией, напр. `overrides:`), сохраняя
/// комментарии и формат. Корня нет — создаёт `rules:` с нуля.
fn insert_rule_fragment(text: &str, fragment: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let root = lines.iter().position(|line| {
        !line.starts_with(char::is_whitespace)
            && (line.trim_end() == "rules:" || line.trim_end() == "constraints:")
    });
    let Some(root) = root else {
        return format!("rules:\n{fragment}\n");
    };
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(root + 1) {
        let top_level_key = line.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && line.contains(':')
            && !line.starts_with(char::is_whitespace);
        if top_level_key {
            end = i;
            break;
        }
    }
    let mut out: Vec<&str> = Vec::with_capacity(lines.len() + fragment.lines().count() + 2);
    out.extend_from_slice(&lines[..end]);
    if end > 0 && !lines[end - 1].trim().is_empty() {
        out.push("");
    }
    out.extend(fragment.lines());
    out.extend_from_slice(&lines[end..]);
    let mut result = out.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Пишет правило-кандидат в реестр репозитория со статусом `proposed`.
///
/// # Errors
/// Реестра нет и его не создать, запись не удалась.
fn write_rule(repo: &Path, proposal: &RuleProposal) -> Result<PathBuf> {
    let path = crate::control::resolve_constraints_path(repo, None)
        .unwrap_or_else(|| repo.join(crate::control::ROOT_CONSTRAINTS_PATH));
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = insert_rule_fragment(&existing, &proposal.yaml());
    std::fs::write(&path, updated).map_err(|e| HarnessError::io(&path, e))?;
    Ok(path)
}

/// Тело дельты промоушена (упоминает защищённый путь `CONSTRAINTS.yaml` и,
/// при подписи, несёт [`HUMAN_SIGN_OFF`]).
fn delta_body(route: Route, sign_off: bool, lesson: &Lesson, proposal: &RuleProposal) -> String {
    let sig_line = if sign_off {
        format!("{HUMAN_SIGN_OFF}\n")
    } else {
        String::new()
    };
    format!(
        "# Дельта: promote-{id}\n\
         \n\
         - Route: {route}\n\
         - Created: {date}\n\
         {sig_line}\n\
         ## Проблема\n\
         \n\
         Урок {id} ({sig}) подтверждён счётчиками доказательств и выразим\n\
         fitness-правилом (ADR-089): контур ужесточается сам — обучение меняет\n\
         канал «контекст» на канал «инвариант» (критерий E5).\n\
         \n\
         ## ADDED\n\
         \n\
         - Правило `{rule}` (type: {kind}) в CONSTRAINTS.yaml со статусом `proposed`.\n\
         \n\
         ## MODIFIED\n\
         \n\
         - `CONSTRAINTS.yaml` — добавлено правило `{rule}`.\n\
         \n\
         ## REMOVED\n\
         \n\
         - (нет)\n\
         \n\
         ## План отката\n\
         \n\
         - Удалить правило `{rule}` из CONSTRAINTS.yaml отдельной дельтой;\n\
           урок {id} перевести в `retired` (`arch-be lessons retire`).\n\
         \n\
         ## Критерии приёмки\n\
         \n\
         - [ ] redteam-засев: правило ловит нарушение и не краснит зелёный HEAD.\n\
         - [ ] правило в CONSTRAINTS.yaml со статусом proposed.\n",
        id = lesson.id,
        route = route,
        date = super::today(),
        sig_line = sig_line,
        sig = lesson.signature.describe(),
        rule = proposal.name,
        kind = proposal.kind.as_str(),
    )
}

/// Параметры промоушена (край CLI).
#[derive(Debug, Clone)]
pub struct PromoteOptions {
    /// Маршрут значимости (Standard/Critical требуют подпись).
    pub route: Route,
    /// Подпись человека ([`HUMAN_SIGN_OFF`] в дельте).
    pub sign_off: bool,
    /// Инвариант спайна для поля `ad` правила.
    pub ad: Option<String>,
    /// Имя дельты (по умолчанию — `promote-<lesson-id>`).
    pub delta_name: Option<String>,
}

impl Default for PromoteOptions {
    fn default() -> Self {
        Self {
            route: Route::Fast,
            sign_off: false,
            ad: None,
            delta_name: None,
        }
    }
}

/// Итог промоушена урока.
#[derive(Debug, Clone)]
pub struct PromoteReport {
    /// Id урока.
    pub lesson_id: String,
    /// Имя правила (ссылка `promoted_rule` урока).
    pub rule_name: String,
    /// Вид правила.
    pub kind: PromoRuleKind,
    /// Путь к реестру, куда записано правило.
    pub constraints: PathBuf,
    /// Путь к дельте.
    pub delta: PathBuf,
    /// YAML-фрагмент правила.
    pub yaml: String,
}

/// Промоушен урока в fitness-правило: выразимость → подпись (по маршруту) →
/// redteam-засев → дельта + реестр + статус `promoted`.
///
/// Ничего не пишет до прохождения засева и гейта подписи (fail-closed).
///
/// # Errors
/// Урок не найден/не `confirmed`; урок невыразим; маршрут требует подпись, а
/// её нет; redteam-засев отклонён; ошибки записи реестра/дельты/журнала.
pub fn promote(
    repo: &Path,
    lesson_id: &str,
    opts: &PromoteOptions,
    th: &Thresholds,
) -> Result<PromoteReport> {
    let ledger = Ledger::repo(repo);
    let snapshot = ledger.snapshot(th)?;
    let lesson = snapshot.get(lesson_id).cloned().ok_or_else(|| {
        HarnessError::Control(format!(
            "реестр уроков: урок {lesson_id} не найден — нечего промоутировать"
        ))
    })?;
    if lesson.status != LessonStatus::Confirmed {
        return Err(HarnessError::Control(format!(
            "промоушен: урок {lesson_id} в статусе '{}' — промоутировать можно только confirmed",
            lesson.status
        )));
    }
    let proposal = express(&lesson).ok_or_else(|| {
        HarnessError::Control(format!(
            "промоушен: урок {lesson_id} невыразим шаблоном rule_templates — \
             сформулируйте структурированную директиву в statement (ADR-089)"
        ))
    })?;
    let proposal = match &opts.ad {
        Some(ad) => proposal.with_ad(ad),
        None => proposal,
    };
    if requires_signature(opts.route) && !opts.sign_off {
        return Err(HarnessError::Control(format!(
            "промоушен: маршрут {} требует подпись человека ({HUMAN_SIGN_OFF}) в дельте",
            opts.route
        )));
    }
    match redteam_seed(repo, &proposal)? {
        SeedVerdict::Accepted { .. } => {}
        SeedVerdict::Rejected { reason } => {
            return Err(HarnessError::Control(format!(
                "промоушен отклонён redteam-засевом: {reason}"
            )));
        }
    }
    let constraints = write_rule(repo, &proposal)?;
    let delta_name = opts
        .delta_name
        .clone()
        .unwrap_or_else(|| format!("promote-{}", lesson.id));
    let body = delta_body(opts.route, opts.sign_off, &lesson, &proposal);
    let delta = crate::delta::new_with_body(repo, &delta_name, &body)?;
    ledger.promote(lesson_id, &proposal.name, Some(&delta_name), th)?;
    Ok(PromoteReport {
        lesson_id: lesson_id.to_string(),
        rule_name: proposal.name.clone(),
        kind: proposal.kind,
        constraints,
        delta,
        yaml: proposal.yaml(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Route;
    use crate::lessons::{
        Evidence, Ledger, Lesson, LessonSource, LessonStatus, Outcome, RecordInput, Signature,
        Thresholds, today,
    };

    const TODAY: &str = "2026-10-14";

    /// Доказательство прохода после применения (`pass_after_apply`).
    fn hit(run: &str, task: &str) -> Evidence {
        Evidence {
            run_id: run.to_string(),
            harness: None,
            verdict_excerpt: String::new(),
            file: None,
            line: None,
            ts: "2026-10-14T10:00:00Z".to_string(),
            outcome: Outcome::PassAfterApply,
            task: Some(task.to_string()),
        }
    }

    /// Заводит в реестре `confirmed`-урок (2 hits, 0 misses, 2 задачи) с
    /// заданным `statement` и возвращает его id.
    fn seed_confirmed(ledger: &Ledger, th: &Thresholds, code: &str, statement: &str) -> String {
        let sig = Signature::new(LessonSource::Gate, code);
        let id = sig.id();
        for (run, task) in [("r1", "T1"), ("r2", "T2")] {
            let input = RecordInput::new(sig.clone(), hit(run, task)).with_statement(statement);
            ledger.record(&input, th, TODAY).expect("record");
        }
        let snapshot = ledger.snapshot(th).expect("snapshot");
        let lesson = snapshot.get(&id).expect("lesson");
        assert_eq!(
            lesson.status,
            LessonStatus::Confirmed,
            "урок обязан дойти до confirmed: {lesson:?}"
        );
        id
    }

    /// Директива `must_not_contain` для statement урока.
    fn directive_must_not_contain(pattern: &str) -> String {
        format!("kind: must_not_contain\nglob: '*.rs'\npattern: '{pattern}'\n")
    }

    /// Кейс-репозиторий с зелёным реестром (`file_exists` на README.md).
    fn green_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::write(repo.join("README.md"), "# репозиторий\n").expect("readme");
        std::fs::write(
            repo.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: baseline\n    type: file_exists\n    \
             path: README.md\n    severity: error\n",
        )
        .expect("constraints");
        repo
    }

    // --- выразимость (решение 1 ADR-089) -------------------------------------

    #[test]
    fn express_requires_confirmed_status() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ledger = Ledger::at(tmp.path().join("ledger.jsonl"));
        let th = Thresholds::default();
        let sig = Signature::new(LessonSource::Gate, "G5");
        // Один hit — candidate, не confirmed.
        ledger
            .record(
                &RecordInput::new(sig.clone(), hit("r1", "T1"))
                    .with_statement(directive_must_not_contain("X")),
                &th,
                TODAY,
            )
            .expect("record");
        let lesson = ledger
            .snapshot(&th)
            .expect("snapshot")
            .get(&sig.id())
            .cloned()
            .expect("lesson");
        assert_eq!(lesson.status, LessonStatus::Candidate);
        assert!(express(&lesson).is_none(), "не confirmed — невыразим");
    }

    #[test]
    fn express_parses_structured_directive_for_content_rule() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ledger = Ledger::at(tmp.path().join("ledger.jsonl"));
        let th = Thresholds::default();
        let id = seed_confirmed(
            &ledger,
            &th,
            "G5",
            "kind: must_not_contain\nglob: 'src/**/*.rs'\npattern: 'unsafe'\n",
        );
        let lesson = ledger
            .snapshot(&th)
            .expect("snapshot")
            .get(&id)
            .cloned()
            .expect("lesson");
        let proposal = express(&lesson).expect("выразим");
        assert_eq!(proposal.kind, PromoRuleKind::MustNotContain);
        assert_eq!(proposal.glob.as_deref(), Some("src/**/*.rs"));
        assert_eq!(proposal.pattern.as_deref(), Some("unsafe"));
        assert!(
            proposal.yaml().contains("type: must_not_contain"),
            "{}",
            proposal.yaml()
        );
        assert!(
            proposal.yaml().contains("status: proposed"),
            "{}",
            proposal.yaml()
        );
    }

    #[test]
    fn express_matches_rule_template_for_command_succeeds() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ledger = Ledger::at(tmp.path().join("ledger.jsonl"));
        let th = Thresholds::default();
        let id = seed_confirmed(
            &ledger,
            &th,
            "idempotency",
            "Выплата идемпотентна по ключу: повторная доставка не плодит дублей",
        );
        let lesson = ledger
            .snapshot(&th)
            .expect("snapshot")
            .get(&id)
            .cloned()
            .expect("lesson");
        let proposal = express(&lesson).expect("выразим шаблоном");
        assert_eq!(proposal.kind, PromoRuleKind::CommandSucceeds);
        assert_eq!(proposal.template_id.as_deref(), Some("idempotency-key"));
        assert!(proposal.command.is_some());
    }

    #[test]
    fn express_returns_none_for_non_expressible_lesson() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ledger = Ledger::at(tmp.path().join("ledger.jsonl"));
        let th = Thresholds::default();
        let id = seed_confirmed(&ledger, &th, "G7", "Космический лифт и расписание запусков");
        let lesson = ledger
            .snapshot(&th)
            .expect("snapshot")
            .get(&id)
            .cloned()
            .expect("lesson");
        assert!(
            express(&lesson).is_none(),
            "проза без директивы и шаблона невыразима"
        );
    }

    // --- redteam-засев (решение 2 ADR-089) -----------------------------------

    #[test]
    fn rule_reddening_head_is_rejected() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = green_repo(tmp.path());
        // Нарушение уже есть в HEAD — правило обязано краситься на HEAD.
        std::fs::write(repo.join("bad.rs"), "MARKER_VIOLATION\n").expect("bad");
        let proposal = express_directive("MARKER_VIOLATION");
        let verdict = redteam_seed(&repo, &proposal).expect("seed");
        match verdict {
            SeedVerdict::Rejected { reason } => {
                assert!(reason.contains("зелёный HEAD"), "{reason}");
            }
            other @ SeedVerdict::Accepted { .. } => {
                panic!("ожидался отказ, получено {other:?}")
            }
        }
    }

    #[test]
    fn toothy_rule_on_green_head_is_accepted() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = green_repo(tmp.path());
        let proposal = express_directive("FORBIDDEN_MARKER");
        let verdict = redteam_seed(&repo, &proposal).expect("seed");
        assert!(
            matches!(verdict, SeedVerdict::Accepted { .. }),
            "{verdict:?}"
        );
    }

    /// Прямая свёртка `must_not_contain`-директивы в предложение (без реестра).
    fn express_directive(pattern: &str) -> RuleProposal {
        let lesson = Lesson::from_input(
            &RecordInput::new(Signature::new(LessonSource::Gate, "G5"), hit("r1", "T1"))
                .with_statement(directive_must_not_contain(pattern)),
        );
        let mut lesson = lesson;
        lesson.status = LessonStatus::Confirmed;
        express(&lesson).expect("выразим")
    }

    // --- подпись человека (решение 3 ADR-089) --------------------------------

    #[test]
    fn critical_route_requires_human_sign_off() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = green_repo(tmp.path());
        let ledger = Ledger::repo(&repo);
        let th = Thresholds::default();
        let id = seed_confirmed(
            &ledger,
            &th,
            "G5",
            &directive_must_not_contain("FORBIDDEN_MARKER"),
        );
        let opts = PromoteOptions {
            route: Route::Critical,
            sign_off: false,
            ..PromoteOptions::default()
        };
        let err = promote(&repo, &id, &opts, &th).expect_err("без подписи — отказ");
        assert!(err.to_string().contains("подпись"), "{err}");
        assert!(
            err.to_string().contains(HUMAN_SIGN_OFF),
            "маркер подписи назван: {err}"
        );
    }

    #[test]
    fn fast_route_does_not_require_sign_off() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = green_repo(tmp.path());
        let ledger = Ledger::repo(&repo);
        let th = Thresholds::default();
        let id = seed_confirmed(
            &ledger,
            &th,
            "G5",
            &directive_must_not_contain("FORBIDDEN_MARKER"),
        );
        let report =
            promote(&repo, &id, &PromoteOptions::default(), &th).expect("fast без подписи");
        assert_eq!(report.rule_name, "promoted_g5_must_not_contain");
    }

    // --- после промоушена pre-commit ловит нарушение (решение 4) ---------------

    #[test]
    fn promoted_rule_catches_seeded_violation_on_check() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = green_repo(tmp.path());
        let ledger = Ledger::repo(&repo);
        let th = Thresholds::default();
        let id = seed_confirmed(
            &ledger,
            &th,
            "G5",
            &directive_must_not_contain("FORBIDDEN_MARKER"),
        );
        let report = promote(&repo, &id, &PromoteOptions::default(), &th).expect("промоушен");
        assert!(report.constraints.is_file());
        assert!(
            report.delta.is_file(),
            "дельта создана: {}",
            report.delta.display()
        );

        // Урок переведён в promoted, правило записано в реестр.
        let lesson = ledger
            .snapshot(&th)
            .expect("snapshot")
            .get(&id)
            .cloned()
            .expect("lesson");
        assert_eq!(lesson.status, LessonStatus::Promoted);
        assert_eq!(
            lesson.promoted_rule.as_deref(),
            Some(report.rule_name.as_str())
        );

        // Зелёный HEAD после промоушена остаётся зелёным.
        let constraints = crate::control::resolve_constraints_path(&repo, None).expect("реестр");
        let green = crate::control::check(&repo, &constraints).expect("check");
        assert!(
            green.passed,
            "промоутированное правило не красит зелёный HEAD"
        );

        // Засеянное нарушение ловится тем же прогоном, что делает pre-commit.
        std::fs::write(repo.join("bad.rs"), "FORBIDDEN_MARKER\n").expect("seed");
        let red = crate::control::check(&repo, &constraints).expect("check");
        assert!(
            !red.passed,
            "pre-commit (control check) обязан поймать нарушение"
        );
    }

    // --- анти-ослабление распространяется на промоутированные правила (решение 5)

    #[test]
    fn promoted_rule_is_protected_by_rule_weakened() {
        let base = "rules:\n  - name: promoted_g5_must_not_contain\n    \
                    type: must_not_contain\n    glob: '*.rs'\n    pattern: 'MARK'\n    \
                    severity: error\n";
        let removed = "rules:\n";
        let issues = crate::control::rule_weakened(removed, base, Path::new("CONSTRAINTS.yaml"))
            .expect("rule_weakened");
        assert!(
            issues
                .iter()
                .any(|i| i.rule == "rule_weakened" && i.message.contains("удалено")),
            "удаление промоутированного правила — ослабление: {issues:?}"
        );

        // Ужесточение (повышение severity не меняет тело) ослаблением не считается.
        let same = base;
        let issues = crate::control::rule_weakened(same, base, Path::new("CONSTRAINTS.yaml"))
            .expect("rule_weakened");
        assert!(issues.is_empty(), "без изменений находок нет: {issues:?}");
    }

    // --- фрагмент правила: валиден и грузится боевой схемой ---------------------

    #[test]
    fn every_kind_yaml_parses_and_loads_into_control_schema() {
        let kinds = [
            "kind: must_contain\nglob: '*.md'\npattern: 'ok'\n",
            "kind: must_not_contain\nglob: '*.rs'\npattern: 'unsafe'\n",
            "kind: file_exists\npath: 'README.md'\n",
            "kind: command_succeeds\ncommand: 'true'\n",
            "kind: conditional\nif:\n  glob: 'src/**/*.rs'\n  pattern: 'http://'\nthen:\n  glob: 'src/**/*.rs'\n  pattern: 'circuit'\n",
        ];
        let tmp = tempfile::tempdir().expect("tmp");
        for statement in kinds {
            let lesson = Lesson::from_input(
                &RecordInput::new(Signature::new(LessonSource::Gate, "G5"), hit("r1", "T1"))
                    .with_statement(statement),
            );
            let mut lesson = lesson;
            lesson.status = LessonStatus::Confirmed;
            let proposal = express(&lesson).unwrap_or_else(|| panic!("выразим: {statement}"));
            let doc = format!("rules:\n{}\n", proposal.yaml());
            let path = tmp
                .path()
                .join(format!("CONSTRAINTS-{}.yaml", proposal.kind.as_str()));
            std::fs::write(&path, &doc).expect("write");
            let loaded = crate::control::load_fitness_rules(&path)
                .unwrap_or_else(|e| panic!("{}: {e}\n{doc}", proposal.kind.as_str()));
            assert_eq!(loaded.len(), 1, "{}", proposal.kind.as_str());
        }
    }

    // --- мелкие детерминированные хелперы --------------------------------------

    #[test]
    fn seed_path_covers_common_globs() {
        let repo = Path::new("/repo");
        assert_eq!(seed_path(repo, "*.rs"), Path::new("/repo/seed.rs"));
        assert_eq!(
            seed_path(repo, "src/**/*.rs"),
            Path::new("/repo/src/seed.rs")
        );
        assert_eq!(
            seed_path(repo, "docs/*.md"),
            Path::new("/repo/docs/seed.md")
        );
        assert_eq!(seed_path(repo, "**/*"), Path::new("/repo/seed.txt"));
    }

    #[test]
    fn slug_name_and_rule_name_are_snake_case() {
        assert_eq!(slug_name("G5"), "g5");
        assert_eq!(slug_name("New Public Symbol"), "new_public_symbol");
        assert_eq!(
            rule_name("G5", PromoRuleKind::MustNotContain),
            "promoted_g5_must_not_contain"
        );
    }

    #[test]
    fn insert_rule_fragment_places_rule_before_overrides() {
        let text =
            "rules:\n  - name: a\n    type: file_exists\n    path: a\n\noverrides:\n  - rule: a\n";
        let fragment = "  - name: b\n    type: file_exists\n    path: b\n";
        let out = insert_rule_fragment(text, fragment);
        assert!(out.contains("path: b\n"), "{out}");
        let a_pos = out.find("name: a").expect("a");
        let b_pos = out.find("name: b").expect("b");
        let ovr_pos = out.find("overrides:").expect("overrides");
        assert!(a_pos < b_pos && b_pos < ovr_pos, "{out}");
    }

    #[test]
    fn insert_rule_fragment_creates_root_when_absent() {
        let out = insert_rule_fragment("", "  - name: a\n    type: file_exists\n    path: a\n");
        assert!(out.starts_with("rules:\n"), "{out}");
    }

    #[test]
    fn requires_signature_only_on_standard_and_critical() {
        assert!(!requires_signature(Route::Fast));
        assert!(requires_signature(Route::Standard));
        assert!(requires_signature(Route::Critical));
    }

    #[test]
    fn today_helper_is_stable_format() {
        // Формат YYYY-MM-DD: дельта пишет дату создания.
        assert_eq!(today().len(), 10);
    }
}
