//! Реестр правил `CONSTRAINTS.yaml` как данные: модель файла
//! ([`ConstraintsFile`]), толерантный разбор (E8: неизвестный `type` — warn,
//! не падение), плоская загрузка ([`load_fitness_rules`]), единый резолвер
//! пути реестра (E2) и построчный разбор манифестов зависимостей для
//! `deny_dependency`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::diff_triggers::diff_regex;
use super::types::{FitnessRule, OverrideEntry, RuleCard, RuleKind, SkippedUnknownRule};
use crate::error::{HarnessError, Result};

/// Auto-набор манифестов для `deny_dependency` (поле `manifests` не задано).
pub(super) const DEFAULT_MANIFEST_GLOBS: [&str; 3] =
    ["**/Cargo.toml", "**/pom.xml", "**/requirements.txt"];

/// Построчный разбор `Cargo.toml`: имена пакетов из секций `[dependencies]`,
/// `[dev-dependencies]`, `[build-dependencies]` (и `*.dependencies` —
/// target-специфичных). Имя — ключ до `=`; значения не разбираются.
/// Возвращает (пакет, строка 1-based).
fn parse_cargo_manifest_deps(content: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut in_deps = false;
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with('[') {
            let section = line.trim_matches(['[', ']']).trim();
            in_deps = section == "dependencies"
                || section == "dev-dependencies"
                || section == "build-dependencies"
                || section.ends_with(".dependencies");
            continue;
        }
        if !in_deps || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = line.split('=').next().unwrap_or_default().trim();
        let name = key
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches('"')
            .trim_matches('\'');
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            out.push((name.to_string(), idx + 1));
        }
    }
    out
}

/// Построчный разбор `pom.xml`: все `<artifactId>` (эвристика MVP: собственный
/// artifactId проекта и parent тоже попадают в список — задокументировано;
/// ложное срабатывание возможно только при совпадении имени проекта с
/// deny-пакетом). Возвращает (пакет, строка 1-based).
fn parse_pom_manifest_deps(content: &str) -> Result<Vec<(String, usize)>> {
    let re = diff_regex(r"<artifactId>\s*([^<]+?)\s*</artifactId>")?;
    let mut out = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        for caps in re.captures_iter(line) {
            out.push((caps[1].to_string(), idx + 1));
        }
    }
    Ok(out)
}

/// Построчный разбор `requirements.txt`: имя пакета — до спецификатора
/// версии (`==`/`>=`/`~=`/`[` и т.п.); комментарии `#`, опции (`-r`, `-e`)
/// и пустые строки пропускаются. Возвращает (пакет, строка 1-based).
fn parse_requirements_manifest_deps(content: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.split(" #").next().unwrap_or_default().trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }
        let name = line
            .split(['=', '<', '>', '!', '~', '[', ';', ' '])
            .next()
            .unwrap_or_default()
            .trim();
        if !name.is_empty() {
            out.push((name.to_string(), idx + 1));
        }
    }
    out
}

/// Зависимости манифеста по имени файла (для `deny_dependency`).
pub(super) fn manifest_deps(rel: &str, content: &str) -> Result<Vec<(String, usize)>> {
    let name = Path::new(rel)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name == "Cargo.toml" {
        Ok(parse_cargo_manifest_deps(content))
    } else if name == "pom.xml" {
        parse_pom_manifest_deps(content)
    } else {
        // requirements.txt и любые иные *.txt по glob — формат pip.
        Ok(parse_requirements_manifest_deps(content))
    }
}

/// Пакетная копия реестра ограничений (handoff-пакет) — приоритетная в
/// резолвере пути (E2).
pub const HANDOFF_CONSTRAINTS_PATH: &str = ".arch-handoff/CONSTRAINTS.yaml";

/// Корневая копия реестра ограничений — fallback резолвера пути (E2, D6).
pub const ROOT_CONSTRAINTS_PATH: &str = "CONSTRAINTS.yaml";

/// Результат резолва пути к реестру ограничений (E2): использованный путь
/// и пометка дрейфа второй копии.
#[derive(Debug, Clone)]
pub struct ConstraintsPathResolution {
    /// Использованный файл ограничений.
    pub path: PathBuf,
    /// Вторая копия реестра: существует и отличается от использованной
    /// (drift). `None` — копия одна, идентична либо путь задан явно.
    pub drift: Option<PathBuf>,
}

impl ConstraintsPathResolution {
    /// Строка-пометка дрейфа для выводов инструментов (E2); `None` — дрейфа
    /// нет.
    #[must_use]
    pub fn drift_note(&self) -> Option<String> {
        self.drift
            .as_ref()
            .map(|other| constraints_drift_note(&self.path, other))
    }
}

/// Единый резолвер пути к реестру ограничений (E2): явный путь →
/// `<repo>/.arch-handoff/CONSTRAINTS.yaml` → fallback `<repo>/CONSTRAINTS.yaml`.
///
/// `None` — нет ни одной копии (явно заданный путь возвращается как есть:
/// его существование проверяет вызывающий — гейту нужен дефолтный путь для
/// честного SKIP). Порядок совпадает с резолвером гейта (`src/gate.rs`).
#[must_use]
pub fn resolve_constraints_path(repo: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    resolve_constraints_path_detailed(repo, explicit).map(|r| r.path)
}

/// Полный вариант [`resolve_constraints_path`] с пометкой дрейфа двух
/// копий: когда существуют ОБЕ (корневая и пакетная) и они различаются —
/// `drift` указывает на вторую (неиспользованную) копию. Сравнение
/// побайтовое: содержимое по правилам не сопоставляется.
#[must_use]
pub fn resolve_constraints_path_detailed(
    repo: &Path,
    explicit: Option<&Path>,
) -> Option<ConstraintsPathResolution> {
    if let Some(p) = explicit {
        return Some(ConstraintsPathResolution {
            path: p.to_path_buf(),
            drift: None,
        });
    }
    let handoff = repo.join(HANDOFF_CONSTRAINTS_PATH);
    let root = repo.join(ROOT_CONSTRAINTS_PATH);
    let handoff_exists = handoff.is_file();
    let root_exists = root.is_file();
    let (path, other) = if handoff_exists {
        (handoff, root)
    } else if root_exists {
        (root, handoff)
    } else {
        return None;
    };
    let drift = if handoff_exists && root_exists && files_differ(&path, &other) {
        Some(other)
    } else {
        None
    };
    Some(ConstraintsPathResolution { path, drift })
}

/// Побайтовое сравнение двух файлов. Прочитать не удалось — дрейф не
/// утверждаем: ложная пометка хуже её отсутствия, а нечитаемый файл всё
/// равно всплывёт ошибкой загрузки.
fn files_differ(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x != y,
        _ => false,
    }
}

/// Строка-пометка дрейфа двух копий реестра (E2):
/// «копии реестра различаются: используется X; Y отличается (drift)».
#[must_use]
pub fn constraints_drift_note(used: &Path, other: &Path) -> String {
    format!(
        "копии реестра различаются: используется {}; {} отличается (drift)",
        used.display(),
        other.display()
    )
}

/// Корень `CONSTRAINTS.yaml`.
#[derive(Debug, Deserialize)]
pub(super) struct ConstraintsFile {
    /// Список правил (канонический корень `rules:`).
    #[serde(default)]
    pub(super) rules: Vec<FitnessRule>,
    /// Альтернативный корень `constraints:` (кейсы и handoff-пакеты).
    #[serde(default)]
    pub(super) constraints: Vec<FitnessRule>,
    /// Наследование (`docs/corp-spine.md`): родительские constraint-файлы
    /// в форме `<ref>@<version>` — правила подмешиваются с меткой
    /// источника, пин версии проверяется против поля `version` родителя.
    #[serde(default)]
    pub(super) extends: Vec<String>,
    /// Исключения правил через ADR (см. [`OverrideEntry`]).
    #[serde(default)]
    pub(super) overrides: Vec<OverrideEntry>,
}

impl ConstraintsFile {
    /// Все правила из обоих допустимых корней.
    pub(super) fn all_rules(&self) -> impl Iterator<Item = &FitnessRule> {
        self.rules.iter().chain(self.constraints.iter())
    }
}

/// Тень корня `CONSTRAINTS.yaml` для толерантного разбора (E8): правила —
/// сырыми значениями, чтобы неизвестный `type` одной записи не ронял весь
/// файл. Остальные поля типизированы: синтаксически битый YAML, битые
/// `extends`/`overrides` — по-прежнему ошибка разбора.
#[derive(Debug, Deserialize)]
pub(super) struct ConstraintsFileShadow {
    /// Канонический корень `rules:` (сырые записи).
    #[serde(default)]
    rules: Vec<serde_yaml_ng::Value>,
    /// Альтернативный корень `constraints:` (сырые записи).
    #[serde(default)]
    constraints: Vec<serde_yaml_ng::Value>,
    /// Наследование (типизировано — ошибки не смягчаются).
    #[serde(default)]
    extends: Vec<String>,
    /// Версия этого файла (обязательна у родительских constraint-файлов —
    /// по ней дочерние проверяют свой пин в `extends`).
    #[serde(default)]
    pub(super) version: Option<String>,
    /// Исключения правил через ADR (типизированы).
    #[serde(default)]
    overrides: Vec<OverrideEntry>,
}

/// Если значение — запись со строковым `type`, неизвестным словарю
/// [`RuleKind`], вернуть (имя правила, тип) для warn-пропуска (E8).
/// `None` — запись не «чужой словарь»: её ошибка разбора остаётся ошибкой
/// (битые поля, `type` не строкой).
fn unknown_rule_type(value: &serde_yaml_ng::Value) -> Option<(String, String)> {
    let rule_type = value.get("type")?.as_str()?;
    let known =
        serde_yaml_ng::from_value::<RuleKind>(serde_yaml_ng::Value::String(rule_type.to_string()))
            .is_ok();
    if known {
        return None;
    }
    let name = value
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("<без имени>");
    Some((name.to_string(), rule_type.to_string()))
}

/// Разбирает список сырых записей правил: валидные — в [`FitnessRule`],
/// записи с неизвестным `type` — в `skipped` (E8). Остальные ошибки разбора
/// записи — ошибка всего файла (строгость к битым правилам сохранена).
fn parse_rules_tolerant(
    values: Vec<serde_yaml_ng::Value>,
    file: &Path,
    skipped: &mut Vec<SkippedUnknownRule>,
) -> Result<Vec<FitnessRule>> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        match serde_yaml_ng::from_value::<FitnessRule>(value.clone()) {
            Ok(rule) => out.push(rule),
            Err(e) => {
                let Some((name, rule_type)) = unknown_rule_type(&value) else {
                    return Err(HarnessError::Control(format!(
                        "{}: правило не разбирается: {e}",
                        file.display()
                    )));
                };
                skipped.push(SkippedUnknownRule { name, rule_type });
            }
        }
    }
    Ok(out)
}

/// Толерантный разбор constraint-файла (E8): неизвестный `type` отдельного
/// правила — warn-пропуск (второй элемент кортежа), а не падение файла;
/// невалидный YAML и битые записи правил — по-прежнему ошибка.
pub(super) fn parse_constraints_file(
    yaml: &str,
    file: &Path,
) -> Result<(ConstraintsFile, Vec<SkippedUnknownRule>)> {
    let shadow: ConstraintsFileShadow = serde_yaml_ng::from_str(yaml)?;
    let mut skipped = Vec::new();
    let rules = parse_rules_tolerant(shadow.rules, file, &mut skipped)?;
    let constraints = parse_rules_tolerant(shadow.constraints, file, &mut skipped)?;
    Ok((
        ConstraintsFile {
            rules,
            constraints,
            extends: shadow.extends,
            overrides: shadow.overrides,
        },
        skipped,
    ))
}

/// Читает и разбирает `CONSTRAINTS.yaml` в список правил (оба корня —
/// `rules:` и `constraints:`). Общий парсер для `check`, `ArchUnit`-моста
/// (ADR-039) и CLI.
///
/// Наследование (`extends`) здесь НЕ разрешается — только плоский файл;
/// полный резолв с метками источника и проверкой пинов — в
/// [`load_constraints_resolved`].
///
/// Правила с неизвестным `type` (словарь другой редакции, E8) пропускаются;
/// отчётность о пропусках — у [`load_fitness_rules_with_skips`] и в
/// warn-находках [`check`]. Здесь пропуски отбрасываются: вызывающим
/// (ArchUnit-мост, fail-soft читатели карточек) нужен только исполняемый
/// плоский список.
///
/// # Errors
/// Файл не читается, YAML невалиден, запись правила бита (кроме случая
/// неизвестного `type` — он пропускается).
pub fn load_fitness_rules(constraints: &Path) -> Result<Vec<FitnessRule>> {
    let (rules, _skipped) = load_fitness_rules_with_skips(constraints)?;
    Ok(rules)
}

/// Вариант [`load_fitness_rules`], возвращающий и пропущенные записи с
/// неизвестным `type` (E8) — для отчётов, которым пропуск нельзя потерять.
///
/// # Errors
/// Те же, что у [`load_fitness_rules`].
pub fn load_fitness_rules_with_skips(
    constraints: &Path,
) -> Result<(Vec<FitnessRule>, Vec<SkippedUnknownRule>)> {
    let yaml =
        std::fs::read_to_string(constraints).map_err(|e| HarnessError::io(constraints, e))?;
    let (parsed, skipped) = parse_constraints_file(&yaml, constraints)?;
    let ConstraintsFile {
        rules, constraints, ..
    } = parsed;
    Ok((rules.into_iter().chain(constraints).collect(), skipped))
}

/// Загружает карточки правил реестра (плоский список: id, имя, тип, команда,
/// задетый инвариант) — общий резолвер для читателей вне `control`.
///
/// # Errors
/// Те же, что у [`load_fitness_rules`].
pub fn rule_cards(constraints: &Path) -> Result<Vec<RuleCard>> {
    Ok(load_fitness_rules(constraints)?
        .into_iter()
        .map(|r| RuleCard {
            id: r.id,
            name: r.name,
            kind: r.kind.as_str(),
            command: r.command,
            ad: r.ad,
            covers: r.covers,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{LintIssue, check};

    // --- E2: единый резолвер пути реестра ограничений + дрейф двух копий ---

    /// Пишет файл в каталог и возвращает его путь (локальный дубль хелпера
    /// из `mod tests` — этот модуль своим набором помощников).
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn resolve_constraints_path_chain() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        // Нет ни одной копии — None.
        assert!(resolve_constraints_path(repo, None).is_none());
        // Только корневая — fallback на неё.
        write_file(repo, "CONSTRAINTS.yaml", "rules: []\n");
        assert_eq!(
            resolve_constraints_path(repo, None),
            Some(repo.join("CONSTRAINTS.yaml"))
        );
        // Пакетная приоритетнее корневой.
        write_file(repo, ".arch-handoff/CONSTRAINTS.yaml", "rules: []\n");
        assert_eq!(
            resolve_constraints_path(repo, None),
            Some(repo.join(".arch-handoff/CONSTRAINTS.yaml"))
        );
        // Явный путь сильнее всего (существование не проверяется).
        let explicit = repo.join("custom.yaml");
        assert_eq!(
            resolve_constraints_path(repo, Some(&explicit)),
            Some(explicit)
        );
    }

    #[test]
    fn resolve_constraints_path_drift_note() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        // Обе копии различаются — дрейф: используется пакетная, корневая отличается.
        write_file(repo, "CONSTRAINTS.yaml", "rules: []\n");
        write_file(
            repo,
            ".arch-handoff/CONSTRAINTS.yaml",
            "rules: []\n# дрейф\n",
        );
        let r = resolve_constraints_path_detailed(repo, None).expect("резолв");
        assert_eq!(r.path, repo.join(".arch-handoff/CONSTRAINTS.yaml"));
        assert_eq!(r.drift, Some(repo.join("CONSTRAINTS.yaml")));
        let note = r.drift_note().expect("пометка дрейфа");
        assert!(note.contains("копии реестра различаются"), "{note}");
        assert!(note.contains("отличается (drift)"), "{note}");
        // Одинаковое содержимое — без пометки.
        write_file(repo, "CONSTRAINTS.yaml", "rules: []\n# дрейф\n");
        let r = resolve_constraints_path_detailed(repo, None).expect("резолв");
        assert_eq!(r.drift, None);
        assert!(r.drift_note().is_none());
        // Одна копия — без пометки.
        std::fs::remove_file(repo.join("CONSTRAINTS.yaml")).unwrap();
        let r = resolve_constraints_path_detailed(repo, None).expect("резолв");
        assert_eq!(r.drift, None);
    }

    // --- E8: неизвестные типы правил — warn-пропуск, не падение парсера ---

    /// Реестр «другой редакции»: два неизвестных типа + одно валидное правило.
    const ALIEN_CONSTRAINTS: &str = "rules:\n\
         \x20 - name: readme\n\
         \x20   type: file_exists\n\
         \x20   path: README.md\n\
         \x20 - name: skill-gate\n\
         \x20   type: skill_contract\n\
         \x20   glob: 'skills/**'\n\
         \x20 - name: addr-trace\n\
         \x20   type: address_trace\n";

    #[test]
    fn check_skips_unknown_rule_types_with_warn() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        write_file(repo, "README.md", "ридми\n");
        let c = write_file(repo, "CONSTRAINTS.yaml", ALIEN_CONSTRAINTS);
        let report = check(repo, &c).expect("чужой реестр не валит прогон");
        assert!(report.passed, "{}", report.summary);
        // Валидное правило исполнилось и прошло; два чужих — warn-находки.
        let skipped: Vec<&LintIssue> = report
            .issues
            .iter()
            .filter(|i| i.rule == "unknown_rule_type")
            .collect();
        assert_eq!(skipped.len(), 2, "{}", report.summary);
        assert!(skipped.iter().all(|i| i.severity == "warn"));
        assert!(
            skipped
                .iter()
                .any(|i| i.message.contains("правило 'skill-gate': неизвестный тип 'skill_contract' — пропущено (словарь другой редакции?)")),
            "{skipped:?}"
        );
        assert_eq!(report.skipped_unknown.len(), 2);
        assert!(
            report
                .summary
                .contains("пропущено правил: 2 (неизвестные типы: address_trace, skill_contract)"),
            "{}",
            report.summary
        );
        // Сводка по-прежнему считает исполняемые правила.
        assert!(report.summary.contains("Правил: 1"), "{}", report.summary);
    }

    #[test]
    fn check_broken_yaml_is_still_error() {
        let dir = tempfile::tempdir().unwrap();
        let c = write_file(dir.path(), "CONSTRAINTS.yaml", "{битый yaml");
        assert!(check(dir.path(), &c).is_err(), "невалидный YAML — ошибка");
    }

    #[test]
    fn check_all_unknown_types_is_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let c = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - name: x\n    type: playbook_graduation\n",
        );
        let err = check(dir.path(), &c).expect_err("исполняемых правил нет");
        assert!(err.to_string().contains("неизвестных типов"), "{err}");
    }

    #[test]
    fn load_fitness_rules_tolerates_unknown_types() {
        let dir = tempfile::tempdir().unwrap();
        let c = write_file(dir.path(), "CONSTRAINTS.yaml", ALIEN_CONSTRAINTS);
        let rules = load_fitness_rules(&c).expect("плоский список");
        assert_eq!(rules.len(), 1);
        let (rules, skipped) = load_fitness_rules_with_skips(&c).expect("с пропусками");
        assert_eq!(rules.len(), 1);
        assert_eq!(skipped.len(), 2);
        assert_eq!(skipped[0].name, "skill-gate");
        assert_eq!(skipped[0].rule_type, "skill_contract");
        // Битая запись (не неизвестный тип) — по-прежнему ошибка.
        let c = write_file(
            dir.path(),
            "BROKEN.yaml",
            "rules:\n  - name: x\n    type: must_contain\n    glob: 123\n",
        );
        assert!(load_fitness_rules(&c).is_err(), "битая запись — ошибка");
    }
}
