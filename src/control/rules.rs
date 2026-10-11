//! Реестр правил `CONSTRAINTS.yaml` как данные: модель файла
//! ([`ConstraintsFile`]), толерантный разбор (E8: неизвестный `type` — warn,
//! не падение), плоская загрузка ([`load_fitness_rules`]), единый резолвер
//! пути реестра (E2) и построчный разбор манифестов зависимостей для
//! `deny_dependency`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::diff_triggers::diff_regex;
use super::types::{DenySpec, FitnessRule, OverrideEntry, RuleCard, RuleKind, SkippedUnknownRule};
use crate::error::{HarnessError, Result};

/// Auto-набор манифестов для `deny_dependency` (поле `manifests` не задано):
/// девять форматов тех-радара банка (Maven, Gradle Groovy/KTS, npm,
/// .NET, Python PEP 621/Poetry, pip, Cargo, Go).
pub(super) const DEFAULT_MANIFEST_GLOBS: [&str; 9] = [
    "**/Cargo.toml",
    "**/pom.xml",
    "**/build.gradle",
    "**/build.gradle.kts",
    "**/package.json",
    "**/*.csproj",
    "**/pyproject.toml",
    "**/requirements.txt",
    "**/go.mod",
];

/// Зависимость манифеста (волна S, ADR-070): общая форма для всех форматов
/// тех-радара. `name` — последний сегмент координаты (артефакт/пакет);
/// `qualified` — полная координата (`groupId:artifactId`, npm/go-путь) для
/// сверки с deny-именем целиком; `line` — строка 1-based; `scope` —
/// нормализованный scope (`compile`/`runtime`/`test`/`peer`/`optional`);
/// `version` — объявленная версия (для диапазона `below`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ManifestDep {
    pub(super) name: String,
    pub(super) qualified: Option<String>,
    pub(super) line: usize,
    pub(super) scope: Option<String>,
    pub(super) version: Option<String>,
}

/// Строка файла (1-based) по байтовому смещению в содержимом.
pub(super) fn line_of(content: &str, offset: usize) -> usize {
    content[..offset.min(content.len())].matches('\n').count() + 1
}

/// Номер строки, где встречается ключ `"<key>"` (best-effort, 1 при неудаче) —
/// для форматов без построчного разбора (JSON/TOML).
fn find_key_line(content: &str, key: &str) -> usize {
    let needle = format!("\"{key}\"");
    content
        .lines()
        .position(|l| l.contains(&needle))
        .map_or(1, |i| i + 1)
}

/// Спецификатор требования pip/PEP 508 → (имя, версия): `h2==2.0.0` → `(h2,
/// 2.0.0)`; `requests>=2.0` → `(requests, 2.0)`; `pkgs` (без ограничения) →
/// `(pkgs, None)`.
fn parse_req_spec(spec: &str) -> (String, Option<String>) {
    let trimmed = spec.trim();
    let cut = trimmed
        .find(['=', '<', '>', '!', '~', '[', ';', ' '])
        .unwrap_or(trimmed.len());
    let name = trimmed[..cut].trim().to_string();
    let rest = trimmed[cut..].trim_start_matches(['=', '<', '>', '!', '~', ' ']);
    let vcut = rest.find(['[', ';', ',', ' ']).unwrap_or(rest.len());
    let version = rest[..vcut].trim();
    (
        name,
        if version.is_empty() {
            None
        } else {
            Some(version.to_string())
        },
    )
}

/// Построчный разбор `Cargo.toml`: имена пакетов из секций `[dependencies]`,
/// `[dev-dependencies]`, `[build-dependencies]` (и `*.dependencies` —
/// target-специфичных). Имя — ключ до `=`; версия — из значения (простая
/// строка `"2.0"` или inline-таблица `{ version = "..." }`).
fn parse_cargo_manifest_deps(content: &str) -> Vec<ManifestDep> {
    let mut out = Vec::new();
    let mut scope: Option<&'static str> = None;
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with('[') {
            let section = line.trim_matches(['[', ']']).trim();
            scope = match section {
                "dependencies" | "build-dependencies" => Some("compile"),
                "dev-dependencies" => Some("test"),
                s if s.ends_with(".dependencies") => Some("compile"),
                _ => None,
            };
            continue;
        }
        let Some(scope) = scope else { continue };
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let name = key
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches('"')
            .trim_matches('\'');
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            continue;
        }
        out.push(ManifestDep {
            name: name.to_string(),
            qualified: None,
            line: idx + 1,
            scope: Some(scope.to_string()),
            version: cargo_value_version(value.trim()),
        });
    }
    out
}

/// Версия из значения Cargo: `"2.0"` → `2.0`; inline-таблица —
/// `version = "..."`.
fn cargo_value_version(value: &str) -> Option<String> {
    if let Some(rest) = value.strip_prefix('"') {
        return rest.split('"').next().map(str::to_string);
    }
    if value.starts_with('{') {
        let re = diff_regex(r#"version\s*=\s*"([^"]+)""#).ok()?;
        return re.captures(value).map(|c| c[1].to_string());
    }
    None
}

/// Нормализация Maven `<scope>`: отсутствие/`compile`/`provided`/`system`/
/// `import` → `compile`; `test` → `test`; `runtime` → `runtime`.
fn normalize_maven_scope(scope: &str) -> &'static str {
    match scope.trim().to_ascii_lowercase().as_str() {
        "test" => "test",
        "runtime" => "runtime",
        _ => "compile",
    }
}

/// Построчный разбор `pom.xml`: зависимости только из блоков `<dependency>…`.
/// Собственный `artifactId` проекта и `<parent>` — НЕ зависимости (S2).
/// `qualified` = `groupId:artifactId`; scope/version — из внутренних тегов.
fn parse_pom_manifest_deps(content: &str) -> Result<Vec<ManifestDep>> {
    let block_re = diff_regex(r"(?s)<dependency>(.*?)</dependency>")?;
    let art_re = diff_regex(r"<artifactId>\s*([^<]+?)\s*</artifactId>")?;
    let group_re = diff_regex(r"<groupId>\s*([^<]+?)\s*</groupId>")?;
    let ver_re = diff_regex(r"<version>\s*([^<]+?)\s*</version>")?;
    let scope_re = diff_regex(r"<scope>\s*([^<]+?)\s*</scope>")?;
    let mut out = Vec::new();
    for block in block_re.captures_iter(content) {
        let body = block.get(1).expect("группа 1 всегда есть");
        let Some(art) = art_re.captures(body.as_str()) else {
            continue;
        };
        let name = art[1].trim().to_string();
        let artifact_at = body.start() + art.get(0).expect("совпадение есть").start();
        let qualified = group_re
            .captures(body.as_str())
            .map(|c| format!("{}:{name}", c[1].trim()));
        let version = ver_re
            .captures(body.as_str())
            .map(|c| c[1].trim().to_string());
        let scope = scope_re
            .captures(body.as_str())
            .map(|c| normalize_maven_scope(&c[1]).to_string());
        out.push(ManifestDep {
            name,
            qualified,
            line: line_of(content, artifact_at),
            scope,
            version,
        });
    }
    Ok(out)
}

/// Scope конфигурации Gradle: `test*` → `test`; `runtime*` → `runtime`;
/// прочее (`implementation`/`api`/`compileOnly`/…) → `compile`.
fn gradle_scope(keyword: &str) -> &'static str {
    let k = keyword.trim().to_ascii_lowercase();
    if k.starts_with("test") {
        "test"
    } else if k.starts_with("runtime") {
        "runtime"
    } else {
        "compile"
    }
}

/// Разбор `build.gradle`/`build.gradle.kts`: строковая нотация
/// (`implementation 'g:a:v'`, в т.ч. `implementation("g:a:v")`) и
/// map-нотация (`implementation group: 'g', name: 'n', version: 'v'`).
/// Не-координатные строки (`apply plugin: 'java'`) пропускаются.
fn parse_gradle_manifest_deps(content: &str) -> Result<Vec<ManifestDep>> {
    let str_re = diff_regex(r#"(?m)([A-Za-z][A-Za-z0-9]*)\s*\(?\s*['"]([^'"\s]+)['"]"#)?;
    let group_re = diff_regex(r#"group:\s*['"]([^'"]+)['"]"#)?;
    let name_re = diff_regex(r#"name:\s*['"]([^'"]+)['"]"#)?;
    let ver_re = diff_regex(r#"version:\s*['"]([^'"]+)['"]"#)?;
    let mut out = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let ln = idx + 1;
        // map-нотация.
        if let (Some(n), Some(g)) = (name_re.captures(line), group_re.captures(line)) {
            let name = n[1].to_string();
            let kw = line.split_whitespace().next().unwrap_or_default();
            out.push(ManifestDep {
                qualified: Some(format!("{}:{name}", &g[1])),
                name,
                line: ln,
                scope: Some(gradle_scope(kw).to_string()),
                version: ver_re.captures(line).map(|c| c[1].to_string()),
            });
            continue;
        }
        // строковая нотация `g:a[:v]`.
        for caps in str_re.captures_iter(line) {
            let mut parts = caps[2].split(':');
            let (Some(group), Some(artifact)) = (parts.next(), parts.next()) else {
                continue;
            };
            out.push(ManifestDep {
                name: artifact.to_string(),
                qualified: Some(format!("{group}:{artifact}")),
                line: ln,
                scope: Some(gradle_scope(&caps[1]).to_string()),
                version: parts.next().map(str::to_string),
            });
        }
    }
    Ok(out)
}

/// Разбор `package.json`: секции `dependencies` (runtime),
/// `devDependencies` (test), `peerDependencies` (peer),
/// `optionalDependencies` (optional). Версия — значение; строка — поиск ключа.
fn parse_package_json_manifest_deps(content: &str) -> Vec<ManifestDep> {
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str(content) else {
        return Vec::new();
    };
    let sections = [
        ("dependencies", "runtime"),
        ("devDependencies", "test"),
        ("peerDependencies", "peer"),
        ("optionalDependencies", "optional"),
    ];
    let mut out = Vec::new();
    for (section, scope) in sections {
        let Some(serde_json::Value::Object(deps)) = root.get(section) else {
            continue;
        };
        for (name, value) in deps {
            out.push(ManifestDep {
                name: name.clone(),
                qualified: None,
                line: find_key_line(content, name),
                scope: Some(scope.to_string()),
                version: value.as_str().map(str::to_string),
            });
        }
    }
    out
}

/// Разбор `*.csproj` (`PackageReference Include="..." Version="..."`).
fn parse_csproj_manifest_deps(content: &str) -> Result<Vec<ManifestDep>> {
    let tag_re = diff_regex(r"<PackageReference\b[^>]*>")?;
    let inc_re = diff_regex(r#"Include\s*=\s*"([^"]+)""#)?;
    let ver_re = diff_regex(r#"Version\s*=\s*"([^"]+)""#)?;
    let mut out = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        for tag in tag_re.find_iter(line) {
            let t = tag.as_str();
            let Some(name) = inc_re.captures(t) else {
                continue;
            };
            out.push(ManifestDep {
                name: name[1].to_string(),
                qualified: None,
                line: idx + 1,
                scope: Some("compile".to_string()),
                version: ver_re.captures(t).map(|c| c[1].to_string()),
            });
        }
    }
    Ok(out)
}

/// Добавить требования из TOML-значения: массив строк (PEP 621
/// `dependencies` / `optional-dependencies`) или таблица `name = version`
/// (Poetry). `python` в Poetry-таблице — не зависимость.
fn push_pyproject_reqs(
    value: Option<&toml::Value>,
    scope: &str,
    content: &str,
    out: &mut Vec<ManifestDep>,
) {
    match value {
        Some(toml::Value::Array(items)) => {
            for item in items {
                if let Some(spec) = item.as_str() {
                    let (name, version) = parse_req_spec(spec);
                    if name.is_empty() {
                        continue;
                    }
                    out.push(ManifestDep {
                        line: find_key_line(content, &name),
                        name,
                        qualified: None,
                        scope: Some(scope.to_string()),
                        version,
                    });
                }
            }
        }
        Some(toml::Value::Table(table)) => {
            for (name, v) in table {
                if name == "python" {
                    continue;
                }
                out.push(ManifestDep {
                    name: name.clone(),
                    qualified: None,
                    line: find_key_line(content, name),
                    scope: Some(scope.to_string()),
                    version: v.as_str().map(str::to_string),
                });
            }
        }
        _ => {}
    }
}

/// Разбор `pyproject.toml`: PEP 621 (`[project] dependencies` и
/// `optional-dependencies`) и Poetry (`[tool.poetry.dependencies]`,
/// `[tool.poetry.group.<g>.dependencies]`). Нечитаемый TOML — без находок.
fn parse_pyproject_manifest_deps(content: &str) -> Vec<ManifestDep> {
    let Ok(value) = content.parse::<toml::Value>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(project) = value.get("project") {
        push_pyproject_reqs(project.get("dependencies"), "compile", content, &mut out);
        if let Some(groups) = project
            .get("optional-dependencies")
            .and_then(toml::Value::as_table)
        {
            for group in groups.values() {
                push_pyproject_reqs(Some(group), "optional", content, &mut out);
            }
        }
    }
    if let Some(poetry) = value.get("tool").and_then(|t| t.get("poetry")) {
        push_pyproject_reqs(poetry.get("dependencies"), "compile", content, &mut out);
        if let Some(groups) = poetry.get("group").and_then(toml::Value::as_table) {
            for group in groups.values() {
                push_pyproject_reqs(group.get("dependencies"), "compile", content, &mut out);
            }
        }
    }
    out
}

/// Разбор `go.mod`: `require <path> <version>` (одиночный) и блок
/// `require ( … )`. `name` — последний сегмент пути, `qualified` — весь путь.
fn parse_go_mod_manifest_deps(content: &str) -> Vec<ManifestDep> {
    let mut out = Vec::new();
    let mut in_block = false;
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("require (") {
            in_block = true;
            continue;
        }
        if in_block && line == ")" {
            in_block = false;
            continue;
        }
        let entry = if in_block {
            line
        } else if let Some(rest) = line.strip_prefix("require ") {
            rest.trim()
        } else {
            continue;
        };
        let mut parts = entry.split_whitespace();
        let Some(path) = parts.next() else {
            continue;
        };
        if path.starts_with("//") || path == "(" || path == ")" {
            continue;
        }
        out.push(ManifestDep {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            qualified: Some(path.to_string()),
            line: idx + 1,
            scope: Some("compile".to_string()),
            version: parts.next().map(str::to_string),
        });
    }
    out
}

/// Построчный разбор `requirements.txt`: имя пакета — до спецификатора
/// версии (`==`/`>=`/`~=`/`[` и т.п.); комментарии `#`, опции (`-r`, `-e`)
/// и пустые строки пропускаются.
fn parse_requirements_manifest_deps(content: &str) -> Vec<ManifestDep> {
    let mut out = Vec::new();
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.split(" #").next().unwrap_or_default().trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }
        let (name, version) = parse_req_spec(line);
        if name.is_empty() {
            continue;
        }
        out.push(ManifestDep {
            name,
            qualified: None,
            line: idx + 1,
            scope: Some("compile".to_string()),
            version,
        });
    }
    out
}

/// Зависимости манифеста по имени файла (для `deny_dependency`, волна S):
/// диспетчер по девяти форматам тех-радара; прочее — формат pip.
pub(super) fn manifest_deps(rel: &str, content: &str) -> Result<Vec<ManifestDep>> {
    let name = Path::new(rel)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name == "Cargo.toml" {
        Ok(parse_cargo_manifest_deps(content))
    } else if name == "pom.xml" {
        parse_pom_manifest_deps(content)
    } else if name == "build.gradle" || name == "build.gradle.kts" {
        parse_gradle_manifest_deps(content)
    } else if name == "package.json" {
        Ok(parse_package_json_manifest_deps(content))
    } else if name == "pyproject.toml" {
        Ok(parse_pyproject_manifest_deps(content))
    } else if name == "go.mod" {
        Ok(parse_go_mod_manifest_deps(content))
    } else if name.ends_with(".csproj") || name.ends_with(".fsproj") || name.ends_with(".vbproj") {
        parse_csproj_manifest_deps(content)
    } else {
        // requirements.txt и любые иные *.txt по glob — формат pip.
        Ok(parse_requirements_manifest_deps(content))
    }
}

/// Версия `candidate` строго ниже порога `threshold` (дот-числовая схема:
/// `2.15.0 < 2.17.1`, `1.9 < 2`; префиксы `^`/`v`/операторов срезаются).
/// Непарсимая версия с ЛЮБОЙ стороны — `false` (fail-open: без версии не
/// красним — иначе ложные находки на `latest`/`*`).
pub(super) fn version_below(candidate: &str, threshold: &str) -> bool {
    fn parts(s: &str) -> Option<Vec<u64>> {
        let start = s.find(|c: char| c.is_ascii_digit())?;
        let mut out = Vec::new();
        for piece in s[start..].split(['.', '-', '+', ' ', '_']) {
            let num: String = piece.chars().take_while(char::is_ascii_digit).collect();
            if num.is_empty() {
                break;
            }
            out.push(num.parse::<u64>().ok()?);
        }
        (!out.is_empty()).then_some(out)
    }
    let (Some(mut a), Some(mut b)) = (parts(candidate), parts(threshold)) else {
        return false;
    };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a < b
}

/// Совпадение записи `deny` с зависимостью (волна S, ADR-070): имя (или
/// полная координата) + диапазон версий (`below`) + фильтр scope.
pub(super) fn deny_spec_matches(spec: &DenySpec, dep: &ManifestDep) -> bool {
    if dep.name != spec.name() && dep.qualified.as_deref() != Some(spec.name()) {
        return false;
    }
    if let Some(below) = spec.below() {
        let Some(version) = dep.version.as_deref() else {
            return false;
        };
        if !version_below(version, below) {
            return false;
        }
    }
    let scopes = spec.scopes();
    if !scopes.is_empty() {
        let scope = dep.scope.as_deref().unwrap_or("compile");
        if !scopes.iter().any(|s| s == scope) {
            return false;
        }
    }
    true
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
            Ok(rule) => {
                // Схема `requires` (неизвестный ресурс — ошибка реестра)
                // проверяется в [`super::exec::check_with_options`]: только там
                // известен реестр зондов конфига `[gate.requires.<имя>]`
                // (ADR-068 Am.3) — плоский разбор конфига не видит.
                out.push(rule);
            }
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

    // --- ADR-068: requires — ресурсы среды правила ---

    /// `requires` читается (строка и список); отсутствие — пусто.
    #[test]
    fn requires_field_parses_string_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let c = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - name: gpu_only\n    type: file_exists\n    path: a\n    requires: [cuda]\n\
             \x20 - name: stand_only\n    type: file_exists\n    path: b\n    requires: stand\n\
             \x20 - name: plain\n    type: file_exists\n    path: c\n",
        );
        let rules = load_fitness_rules(&c).expect("разбор");
        assert_eq!(rules[0].requires, vec!["cuda".to_string()]);
        assert_eq!(rules[1].requires, vec!["stand".to_string()]);
        assert!(rules[2].requires.is_empty(), "нет requires — пусто");
    }

    /// Плоский разбор схемы `requires` ресурсы НЕ валидирует: реестр зондов
    /// живёт в конфиге `[gate.requires.<имя>]` и известен только движку
    /// исполнения (ADR-068 Am.3) — там и проверяется (тест `exec`).
    #[test]
    fn parse_accepts_any_requires_resource() {
        let dir = tempfile::tempdir().unwrap();
        let c = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - name: db_rule\n    type: file_exists\n    path: a\n    requires: [oracle-client]\n",
        );
        let rules = load_fitness_rules(&c).expect("разбор схемы");
        assert_eq!(rules[0].requires, vec!["oracle-client".to_string()]);
    }

    // --- S2 (волна S, ADR-070): тех-радар — форматы/диапазоны/scope ---

    /// Восемь форматов манифестов тех-радара (метрика 10): один запрещённый
    /// пакет `h2` объявлен по-разному — находка обязана появиться в каждом.
    #[test]
    fn deny_dependency_detects_all_manifest_formats() {
        use std::collections::BTreeSet;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let files: &[(&str, &str)] = &[
            (
                "svc-maven/pom.xml",
                "<project><dependencies><dependency><groupId>com.h2database</groupId><artifactId>h2</artifactId></dependency></dependencies></project>\n",
            ),
            (
                "svc-gradle/build.gradle",
                "dependencies {\n  implementation 'com.h2database:h2:2.2.224'\n}\n",
            ),
            (
                "svc-gradle-kts/build.gradle.kts",
                "dependencies {\n  implementation(\"com.h2database:h2:2.2.224\")\n}\n",
            ),
            (
                "svc-node/package.json",
                "{\"name\":\"svc\",\"dependencies\":{\"h2\":\"^2.0.0\"}}\n",
            ),
            (
                "svc-dotnet/svc.csproj",
                "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><PackageReference Include=\"h2\" Version=\"2.0.0\" /></ItemGroup></Project>\n",
            ),
            (
                "svc-python/pyproject.toml",
                "[project]\nname = \"svc\"\ndependencies = [\"h2\"]\n",
            ),
            ("svc-req/requirements.txt", "h2==2.0.0\n"),
            (
                "svc-rust/Cargo.toml",
                "[package]\nname = \"svc\"\nversion = \"0.1.0\"\n\n[dependencies]\nh2 = \"2.0\"\n",
            ),
            (
                "svc-go/go.mod",
                "module example.com/svc\n\ngo 1.21\n\nrequire github.com/example/h2 v1.0.0\n",
            ),
        ];
        for (rel, content) in files {
            write_file(repo, rel, content);
        }
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: probe\n    type: deny_dependency\n    deny: [h2]\n    severity: error\n",
        );
        let report = check(repo, &constraints).unwrap();
        let hit: BTreeSet<String> = report
            .issues
            .iter()
            .filter(|i| i.rule == "probe")
            .map(|i| i.file.display().to_string())
            .collect();
        for (rel, _) in files {
            assert!(
                hit.contains(*rel),
                "формат не распознан: {rel}; находки: {hit:?}"
            );
        }
    }

    /// Диапазон версий (S2, `log4j-core < 2.17.1`): находка ниже порога,
    /// тишина на пороге и выше; версия без диапазона не сравнивается.
    #[test]
    fn deny_dependency_version_range() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        write_file(
            repo,
            "old/pom.xml",
            "<project><dependency><groupId>org.apache.logging.log4j</groupId><artifactId>log4j-core</artifactId><version>2.15.0</version></dependency></project>\n",
        );
        write_file(
            repo,
            "edge/pom.xml",
            "<project><dependency><groupId>org.apache.logging.log4j</groupId><artifactId>log4j-core</artifactId><version>2.17.1</version></dependency></project>\n",
        );
        write_file(
            repo,
            "new/pom.xml",
            "<project><dependency><groupId>org.apache.logging.log4j</groupId><artifactId>log4j-core</artifactId><version>2.20.0</version></dependency></project>\n",
        );
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: no_old_log4j\n    type: deny_dependency\n    deny:\n      - name: log4j-core\n        below: \"2.17.1\"\n    severity: error\n",
        );
        let report = check(repo, &constraints).unwrap();
        let hits: Vec<&LintIssue> = report
            .issues
            .iter()
            .filter(|i| i.rule == "no_old_log4j")
            .collect();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].file, Path::new("old/pom.xml"));
        assert!(hits[0].message.contains("2.15.0"), "{}", hits[0].message);
        assert!(hits[0].message.contains("2.17.1"), "{}", hits[0].message);
    }

    /// Scope-фильтр (S2): `testImplementation`-зависимость не краснит правило
    /// с `scopes: [runtime, compile]`, `implementation` — краснит.
    #[test]
    fn deny_dependency_scope_filter() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        write_file(
            repo,
            "svc-test/build.gradle",
            "dependencies {\n  testImplementation 'com.h2database:h2:2.2.224'\n}\n",
        );
        write_file(
            repo,
            "svc-prod/build.gradle",
            "dependencies {\n  implementation 'com.h2database:h2:2.2.224'\n}\n",
        );
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: no_h2_runtime\n    type: deny_dependency\n    deny:\n      - name: h2\n        scopes: [runtime, compile]\n    severity: error\n",
        );
        let report = check(repo, &constraints).unwrap();
        let hits: Vec<&LintIssue> = report
            .issues
            .iter()
            .filter(|i| i.rule == "no_h2_runtime")
            .collect();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].file, Path::new("svc-prod/build.gradle"));
    }

    /// Точный `pom.xml` (S2): собственный `artifactId` проекта и `<parent>` —
    /// НЕ зависимости; находка только на `<dependency>`-блоке.
    #[test]
    fn deny_dependency_pom_ignores_own_artifact_and_parent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        write_file(
            repo,
            "s/pom.xml",
            "<project>\n  <parent><artifactId>h2-parent</artifactId></parent>\n  <artifactId>h2</artifactId>\n  <dependencies>\n    <dependency><artifactId>ok-lib</artifactId></dependency>\n  </dependencies>\n</project>\n",
        );
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: no_h2\n    type: deny_dependency\n    deny: [h2, h2-parent]\n    severity: error\n",
        );
        let report = check(repo, &constraints).unwrap();
        assert!(
            report.issues.iter().all(|i| i.rule != "no_h2"),
            "{:?}",
            report.issues
        );
    }

    /// Разбор `deny` (S2): одиночная строка, список строк и список
    /// развёрнутых записей `{name, below, scopes}`.
    #[test]
    fn deny_spec_parses_all_forms() {
        let dir = tempfile::tempdir().unwrap();
        let c = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: one\n    type: deny_dependency\n    deny: h2\n\
             \x20 - name: list\n    type: deny_dependency\n    deny: [h2, hsqldb]\n\
             \x20 - name: detailed\n    type: deny_dependency\n    deny:\n      - name: log4j-core\n        below: \"2.17.1\"\n        scopes: [runtime, compile]\n",
        );
        let rules = load_fitness_rules(&c).expect("разбор deny");
        assert_eq!(rules[0].deny.len(), 1);
        assert_eq!(rules[0].deny[0].name(), "h2");
        assert!(rules[0].deny[0].below().is_none());
        assert_eq!(rules[1].deny.len(), 2);
        assert_eq!(rules[2].deny[0].name(), "log4j-core");
        assert_eq!(rules[2].deny[0].below(), Some("2.17.1"));
        assert_eq!(
            rules[2].deny[0].scopes(),
            &["runtime".to_string(), "compile".to_string()]
        );
    }

    /// Компаратор версий (S2): дот-числовая схема, операторы диапазонов
    /// срезаются, непарсимое — НЕ ниже (fail-open: без версии не красним).
    #[test]
    fn version_below_dotted_numeric() {
        assert!(version_below("2.15.0", "2.17.1"));
        assert!(version_below("2.0.0", "2.17.1"));
        assert!(version_below("1.9", "2"));
        assert!(version_below("^2.15.0", "2.17.1"));
        assert!(version_below("v2.15.0", "2.17.1"));
        assert!(!version_below("2.17.1", "2.17.1"));
        assert!(!version_below("2.20.0", "2.17.1"));
        assert!(!version_below("latest", "2.17.1"));
        assert!(!version_below("2.15.0", "legacy"));
    }
}
