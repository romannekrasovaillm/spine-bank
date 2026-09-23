//! Агентные инструменты домена `control` (реестр [`tools`]): `adr_new`,
//! `spine_lint`, `fitness_check`, `significance_score`, `rules_report`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::diff_triggers::{
    significance_score_with_limits, unknown_trigger_names, unknown_triggers_error,
};
use super::exec::check;
use super::report::{lint_spine, rules_report};
use super::rules::{
    HANDOFF_CONSTRAINTS_PATH, ROOT_CONSTRAINTS_PATH, load_fitness_rules_with_skips,
    resolve_constraints_path_detailed,
};
use super::templates::adr_new_with_author;
use super::types::SkippedUnknownRule;
use crate::error::Result;
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Инструменты домена: `adr_new`, `spine_lint`, `fitness_check`, `significance_score`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(AdrNewTool),
        Arc::new(SpineLintTool),
        Arc::new(FitnessCheckTool),
        Arc::new(SignificanceScoreTool),
        Arc::new(RulesReportTool),
    ]
}

/// Инструмент `rules_report`: отчёт по реестру правил `CONSTRAINTS.yaml`
/// (markdown + счётчики; мост в MCP, транш 2 инверсии; read-only).
pub struct RulesReportTool;

#[derive(Debug, Deserialize)]
struct RulesReportArgs {
    /// Корень репозитория.
    #[serde(alias = "path")]
    repo: String,
    /// Путь к `CONSTRAINTS.yaml` (дефолт `<repo>/.arch-handoff/CONSTRAINTS.yaml`,
    /// иначе `<repo>/CONSTRAINTS.yaml`).
    constraints: Option<String>,
}

#[async_trait]
impl Tool for RulesReportTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rules_report".into(),
            description: "Отчёт по реестру правил CONSTRAINTS.yaml: сводка (всего/по типам/по \
                          severity), таблица карточек (owner, expiry, exclude_glob, \
                          effort_hours), находки (правила без owner/expiry, просроченные, \
                          с exclude_glob), git-прокси стоимости сопровождения. Ответ — JSON: \
                          счётчики rules_total/by_kind/by_severity + summary + \
                          report_markdown. Отчёт, а не гейт: passed всегда true"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень репозитория"},
                    "constraints": {
                        "type": "string",
                        "description": "Путь к CONSTRAINTS.yaml (по умолчанию <repo>/.arch-handoff/CONSTRAINTS.yaml, иначе <repo>/CONSTRAINTS.yaml)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: RulesReportArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "rules_report: невалидные аргументы: {e}"
                )));
            }
        };
        let repo = ctx.resolve(&args.repo);
        // Единый резолвер реестра (E2): явный путь → пакетная копия →
        // корневой fallback; дрейф двух копий — пометкой в ответе.
        let explicit = args.constraints.map(|c| ctx.resolve(c));
        let Some(resolution) = resolve_constraints_path_detailed(&repo, explicit.as_deref()) else {
            return Ok(ToolOutput::err(format!(
                "rules_report: реестр ограничений не найден: ни {HANDOFF_CONSTRAINTS_PATH}, ни {ROOT_CONSTRAINTS_PATH} в {}",
                repo.display()
            )));
        };
        let constraints = resolution.path.clone();
        let report = match rules_report(&repo, &constraints) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("rules_report: {e}"))),
        };
        // Счётчики — из того же файла ограничений (тот же разбор, что и в
        // rules_report; отчёт выше уже доказал, что YAML валиден). Записи с
        // неизвестными типами (E8) идут отдельным списком skipped_unknown.
        let (mut rules_total, mut by_kind, mut by_severity) =
            (0usize, BTreeMap::new(), BTreeMap::new());
        let mut skipped_unknown: Vec<SkippedUnknownRule> = Vec::new();
        // Повторное чтение уже проверенного файла не падает; при гонке
        // (файл изменён между вызовами) счётчики остаются нулевыми — отчёт
        // markdown всё равно отдаётся.
        if let Ok((rules, skipped)) = load_fitness_rules_with_skips(&constraints) {
            for r in &rules {
                rules_total += 1;
                *by_kind.entry(r.kind.as_str().to_string()).or_insert(0usize) += 1;
                *by_severity.entry(r.severity.clone()).or_insert(0usize) += 1;
            }
            skipped_unknown = skipped;
        }
        let drift_note = resolution.drift_note();
        let summary = format!(
            "Реестр правил {}: {rules_total} правил{}; отчёт markdown в поле report_markdown",
            constraints.display(),
            if skipped_unknown.is_empty() {
                String::new()
            } else {
                format!(", пропущено с неизвестным типом: {}", skipped_unknown.len())
            }
        );
        let verdict = json!({
            "tool": "rules_report",
            "passed": true,
            "constraints": constraints.display().to_string(),
            "drift_note": drift_note,
            "rules_total": rules_total,
            "by_kind": by_kind,
            "by_severity": by_severity,
            "skipped_unknown": skipped_unknown,
            "summary": summary,
            "report_markdown": report,
        });
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}

/// Инструмент `adr_new`: создать ADR по шаблону AI-DLC с очередным номером.
pub struct AdrNewTool;

#[derive(Debug, Deserialize)]
struct AdrNewArgs {
    /// Заголовок решения.
    title: String,
    /// Каталог ADR (дефолт `docs/adr`).
    #[serde(alias = "path")]
    dir: Option<String>,
    /// Модель-автор документа (J3, ADR-048): пишется в шапку
    /// (`- Модель-автор: …`), чтобы судья брал автора из документа, а не со
    /// слов в момент судейства. `human` — документ пишет человек.
    author_model: Option<String>,
}

#[async_trait]
impl Tool for AdrNewTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "adr_new".into(),
            description: "Создать новый ADR (Architecture Decision Record) по шаблону AI-DLC \
                          (Context/Decision/Alternatives/Consequences/Reversibility) с очередным \
                          номером ADR-NNN в каталоге"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Заголовок решения"},
                    "path": {"type": "string", "description": "Каталог ADR (по умолчанию docs/adr)"},
                    "author_model": {"type": "string", "description": "Модель-автор документа (в шапку ADR; human — писал человек)"}
                },
                "required": ["title"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: AdrNewArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "adr_new: невалидные аргументы: {e}"
                )));
            }
        };
        let dir = ctx.resolve(args.dir.as_deref().unwrap_or("docs/adr"));
        match adr_new_with_author(&dir, &args.title, args.author_model.as_deref()) {
            Ok(path) => Ok(ToolOutput::ok(format!("ADR создан: {}", path.display()))),
            Err(e) => Ok(ToolOutput::err(format!("adr_new: {e}"))),
        }
    }
}

/// Инструмент `spine_lint`: линтер ARCHITECTURE-SPINE.md.
pub struct SpineLintTool;

#[derive(Debug, Deserialize)]
struct SpineLintArgs {
    /// Путь к файлу spine.
    path: String,
}

#[async_trait]
impl Tool for SpineLintTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spine_lint".into(),
            description: "Проверить ARCHITECTURE-SPINE.md: дубли AD-id, пустые/отсутствующие \
                          Binds/Prevents/Rule, заглушки (TODO/TBD), непиннутые версии, \
                          ссылки на несуществующие AD"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Путь к ARCHITECTURE-SPINE.md"}
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: SpineLintArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "spine_lint: невалидные аргументы: {e}"
                )));
            }
        };
        let path = ctx.resolve(&args.path);
        match lint_spine(&path) {
            Ok(issues) if issues.is_empty() => Ok(ToolOutput::ok("spine: нарушений нет")),
            Ok(issues) => {
                let mut out = format!("spine: {} находок\n", issues.len());
                for i in &issues {
                    let _ = writeln!(
                        out,
                        "[{}] {}:{} {} — {}",
                        i.severity,
                        i.file.display(),
                        i.line,
                        i.rule,
                        i.message
                    );
                }
                Ok(ToolOutput::ok(out))
            }
            Err(e) => Ok(ToolOutput::err(format!("spine_lint: {e}"))),
        }
    }
}

/// Инструмент `fitness_check`: прогон fitness functions из `CONSTRAINTS.yaml`.
pub struct FitnessCheckTool;

#[derive(Debug, Deserialize)]
struct FitnessCheckArgs {
    /// Корень репозитория.
    #[serde(alias = "path")]
    repo: String,
    /// Путь к `CONSTRAINTS.yaml` (дефолт `<repo>/.arch-handoff/CONSTRAINTS.yaml`,
    /// иначе `<repo>/CONSTRAINTS.yaml`).
    constraints: Option<String>,
}

#[async_trait]
impl Tool for FitnessCheckTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fitness_check".into(),
            description: "Прогнать fitness functions из CONSTRAINTS.yaml по репозиторию: \
                          must_contain / must_not_contain (regex по glob-набору файлов), \
                          each_file_must_contain (regex в КАЖДОМ файле набора), \
                          file_exists, dir_must_have_file (обязательный файл в каждом каталоге набора), \
                          max_age (свежесть файла), command_succeeds (с таймаутом), \
                          dependency_direction (направление зависимостей: импорты против forbid/allow), \
                          context_boundary (границы контекстов CMP по code_roots модели), \
                          archunit (JVM-гейт: java-правила файла исполняются настоящим ArchUnit, ADR-039). \
                          Итог PASS/FAIL + находки"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень репозитория"},
                    "constraints": {
                        "type": "string",
                        "description": "Путь к CONSTRAINTS.yaml (по умолчанию <repo>/.arch-handoff/CONSTRAINTS.yaml, иначе <repo>/CONSTRAINTS.yaml)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: FitnessCheckArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "fitness_check: невалидные аргументы: {e}"
                )));
            }
        };
        let repo = ctx.resolve(&args.repo);
        // Единый резолвер реестра (E2): явный путь → пакетная копия →
        // корневой fallback; ни одной копии — канонический дефолт, чтобы
        // ошибка «файл не читается» ссылалась на пакетный путь.
        let explicit = args.constraints.map(|c| ctx.resolve(c));
        let resolution = resolve_constraints_path_detailed(&repo, explicit.as_deref());
        let constraints = resolution
            .as_ref()
            .map_or_else(|| repo.join(HANDOFF_CONSTRAINTS_PATH), |r| r.path.clone());
        let drift_note = resolution.and_then(|r| r.drift_note());
        // Прогон может занимать минуты (command_succeeds) — уводим с worker'а runtime.
        match tokio::task::spawn_blocking(move || check(&repo, &constraints)).await {
            Ok(Ok(report)) => {
                let mut out = String::new();
                if let Some(note) = drift_note {
                    let _ = writeln!(out, "Внимание: {note}");
                }
                let _ = writeln!(out, "{}", report.summary);
                for i in &report.issues {
                    let _ = writeln!(
                        out,
                        "  [{}] {}:{} {} — {}",
                        i.severity,
                        i.file.display(),
                        i.line,
                        i.rule,
                        i.message
                    );
                }
                let _ = writeln!(out, "Итог: {}", if report.passed { "PASS" } else { "FAIL" });
                Ok(ToolOutput::ok(out))
            }
            Ok(Err(e)) => Ok(ToolOutput::err(format!("fitness_check: {e}"))),
            Err(e) => Ok(ToolOutput::err(format!(
                "fitness_check: задача прервана: {e}"
            ))),
        }
    }
}

/// Инструмент `significance_score`: Architecture Significance Score → маршрут.
pub struct SignificanceScoreTool;

#[derive(Debug, Deserialize)]
struct SignificanceScoreArgs {
    /// Карта «триггер → сработал».
    triggers: BTreeMap<String, bool>,
}

#[async_trait]
impl Tool for SignificanceScoreTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "significance_score".into(),
            description: "Оценить Architecture Significance Score по 15 триггерам и вернуть \
                          маршрут изменения: Fast, Standard или Critical (пороги — секция \
                          [significance] конфига, дефолт Fast 0–1 / Standard 2–4 / Critical 5+; \
                          критические триггеры security_boundary_change / irreversible_migration / \
                          criticality_or_exception форсируют Critical и не конфигурируются)"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "triggers": {
                        "type": "object",
                        "description": "Карта «триггер → true/false», ключи — из 15 канонических триггеров; незнакомое имя — ошибка инструмента с перечнем канонических (в счёт не идёт)",
                        "additionalProperties": {"type": "boolean"}
                    }
                },
                "required": ["triggers"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: SignificanceScoreArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "significance_score: невалидные аргументы: {e}"
                )));
            }
        };
        // Пороги маршрутов — из конфига ([significance], ADR-034); невалидные
        // границы — понятная ошибка инструмента, не паника и не тихий дефолт.
        let (fast_max, standard_max) = match ctx.config.significance.limits() {
            Ok(l) => l,
            Err(e) => return Ok(ToolOutput::err(format!("significance_score: {e}"))),
        };
        // T-04: незнакомое имя — ошибка, а не подсветка после счёта. Раньше
        // выдуманный триггер увеличивал score и лишь потом назывался «вне
        // канонических», то есть маршрут уже был завышен.
        let unknown = unknown_trigger_names(&args.triggers);
        if !unknown.is_empty() {
            return Ok(ToolOutput::err(format!(
                "significance_score: {}",
                unknown_triggers_error(&unknown)
            )));
        }
        let s = significance_score_with_limits(&args.triggers, fast_max, standard_max);
        let fired = if s.fired.is_empty() {
            "нет".to_string()
        } else {
            s.fired.join(", ")
        };
        let out = format!(
            "Score: {} → маршрут {:?}; сработали: {fired}",
            s.score, s.route
        );
        Ok(ToolOutput::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// Пишет файл в каталог и возвращает его путь.
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }
    #[test]
    fn tools_expose_five_domain_specs() {
        let mut names: Vec<String> = tools().iter().map(|t| t.spec().name.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "adr_new",
                "fitness_check",
                "rules_report",
                "significance_score",
                "spine_lint"
            ]
        );
    }

    #[tokio::test]
    async fn significance_tool_scores_via_call() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let tool = SignificanceScoreTool;
        let out = tool
            .call(
                json!({"triggers": {"new_component": true, "new_vendor": true}}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!out.is_error, "{:?}", out.content);
        assert!(out.content.contains("Standard"), "{}", out.content);
        assert!(out.content.contains("new_component"), "{}", out.content);

        // T-04: незнакомое имя — ошибка, маршрута нет.
        let out = tool
            .call(
                json!({"triggers": {"new_component": true, "exotic": true}}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.is_error, "{:?}", out.content);
        assert!(
            out.content.contains("exotic") && out.content.contains("Канонические"),
            "незнакомый триггер назван с перечнем канонических: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn spine_tool_reports_findings() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "SPINE.md", "### AD-1. X\n- Binds:\n");
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let tool = SpineLintTool;
        let out = tool.call(json!({"path": "SPINE.md"}), &ctx).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("empty_field"), "{}", out.content);
        let err = tool.call(json!({"path": "nope.md"}), &ctx).await.unwrap();
        assert!(err.is_error, "несуществующий файл — is_error");
    }

    #[tokio::test]
    async fn significance_tool_uses_config_thresholds() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::Config::default();
        cfg.significance.fast_max = 2;
        cfg.significance.standard_max = 5;
        let ctx = ToolContext::new(dir.path().to_path_buf(), Arc::new(cfg));
        let tool = SignificanceScoreTool;
        // 2 триггера при fast_max=2 — Fast (при дефолте был бы Standard).
        let out = tool
            .call(
                json!({"triggers": {"new_component": true, "new_vendor": true}}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!out.is_error, "{:?}", out.content);
        assert!(out.content.contains("Fast"), "{}", out.content);

        // Невалидные пороги — ошибка инструмента с понятным текстом.
        let mut cfg = crate::config::Config::default();
        cfg.significance.fast_max = 5;
        cfg.significance.standard_max = 5;
        let ctx = ToolContext::new(dir.path().to_path_buf(), Arc::new(cfg));
        let out = tool
            .call(json!({"triggers": {"new_component": true}}), &ctx)
            .await
            .unwrap();
        assert!(out.is_error, "{:?}", out.content);
        assert!(out.content.contains("fast_max"), "{}", out.content);
    }

    /// Инструмент `rules_report`: счётчики + markdown-отчёт на фикстуре
    /// карточек правил; битый репозиторий — мягкая ошибка.
    #[tokio::test]
    async fn rules_report_tool_counts_and_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            dir.path().join("CONSTRAINTS.yaml"),
            "rules:\n\
             \x20 - name: no-pan\n\
             \x20   type: must_not_contain\n\
             \x20   glob: 'src/**/*.py'\n\
             \x20   pattern: '\\b\\d{16}\\b'\n\
             \x20 - name: readme\n\
             \x20   type: file_exists\n\
             \x20   path: README.md\n\
             \x20   severity: warn\n",
        )
        .unwrap();
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = RulesReportTool
            .call(
                json!({"repo": "repo", "constraints": "CONSTRAINTS.yaml"}),
                &ctx,
            )
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: serde_json::Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["tool"], "rules_report");
        // Отчёт, а не гейт: passed всегда true.
        assert_eq!(v["passed"], true, "{v}");
        assert_eq!(v["rules_total"], 2, "{v}");
        assert_eq!(v["by_kind"]["must_not_contain"], 1, "{v}");
        assert_eq!(v["by_severity"]["warn"], 1, "{v}");
        assert!(
            v["report_markdown"]
                .as_str()
                .expect("md")
                .contains("| no-pan | must_not_contain |"),
            "{v}"
        );
        // Недоступный репозиторий — мягкая ошибка инструмента.
        let out = RulesReportTool
            .call(
                json!({"repo": "missing", "constraints": "CONSTRAINTS.yaml"}),
                &ctx,
            )
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
    }

    /// `rules_report` по умолчанию читает корневой реестр, когда пакетной
    /// копии нет (E2, fallback); дрейф двух копий — пометка в JSON.
    #[tokio::test]
    async fn rules_report_tool_root_fallback_and_drift_note() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        write_file(
            &repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: readme\n    type: file_exists\n    path: README.md\n",
        );
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = RulesReportTool
            .call(json!({"repo": "repo"}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: serde_json::Value = serde_json::from_str(&out.content).expect("JSON");
        assert!(
            v["constraints"]
                .as_str()
                .expect("constraints")
                .ends_with("repo/CONSTRAINTS.yaml"),
            "{v}"
        );
        assert_eq!(v["drift_note"], serde_json::Value::Null, "{v}");
        // Появилась пакетная копия с другим содержимым — пометка дрейфа,
        // используется пакетная.
        write_file(
            &repo,
            ".arch-handoff/CONSTRAINTS.yaml",
            "rules:\n  - name: readme\n    type: file_exists\n    path: README.md\n    severity: warn\n",
        );
        let out = RulesReportTool
            .call(json!({"repo": "repo"}), &ctx)
            .await
            .expect("вызов");
        let v: serde_json::Value = serde_json::from_str(&out.content).expect("JSON");
        let note = v["drift_note"].as_str().expect("drift_note");
        assert!(note.contains("копии реестра различаются"), "{v}");
        assert!(
            v["constraints"]
                .as_str()
                .expect("constraints")
                .contains(".arch-handoff/CONSTRAINTS.yaml"),
            "{v}"
        );
        // Нет ни одной копии — понятная мягкая ошибка.
        let out = RulesReportTool
            .call(json!({"repo": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("реестр ограничений не найден"),
            "{}",
            out.content
        );
    }
}
