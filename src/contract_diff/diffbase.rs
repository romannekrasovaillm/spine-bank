//! Точка входа диффа контрактов: чтение файлов, разбор JSON/YAML,
//! диспетчер по форматам, правило major (CD-007/CD-P06), связка с моделью
//! (ADR-035).

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use crate::error::{HarnessError, Result};
use crate::model::{EntityKind, load_model};

use super::avro::diff_avro;
use super::ddl::diff_ddl;
use super::detect::detect_format;
use super::jsonschema::diff_jsonschema;
use super::openapi::diff_openapi;
use super::proto::{diff_proto, parse_proto};
use super::types::{ContractFormat, ContractImpact, DiffReport, Finding};

// ---------------------------------------------------------------------------
// Точка входа и детектор формата
// ---------------------------------------------------------------------------

/// Читает файл контракта текстом.
///
/// # Errors
/// Файл не читается.
fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))
}

/// Разбирает текст контракта: `{` в начале — JSON, иначе YAML.
///
/// # Errors
/// Содержимое не парсится выбранным форматом.
pub(crate) fn parse_contract(content: &str, path: &Path) -> Result<Value> {
    let trimmed = content.trim_start();
    if trimmed.starts_with('{') {
        serde_json::from_str(trimmed)
            .map_err(|e| HarnessError::Tool(format!("{}: невалидный JSON: {e}", path.display())))
    } else {
        serde_yaml_ng::from_str(trimmed)
            .map_err(|e| HarnessError::Tool(format!("{}: невалидный YAML: {e}", path.display())))
    }
}

/// Сравнивает два контракта: авто-детект формата, без связки с моделью.
///
/// Совместимость транша T1: пара OpenAPI-документов ведёт себя ровно как
/// раньше (те же CD-001..CD-006; CD-007 добавлен п.14 — см. заголовок
/// модуля).
///
/// # Errors
/// Файл не читается, формат не распознан, форматы файлов разные.
pub fn diff_contracts(old: &Path, new: &Path) -> Result<Vec<Finding>> {
    Ok(diff_report(old, new, None, None)?.findings)
}

/// Полный дифф: явный формат (`None` — авто-детект) + опциональная связка
/// с моделью (`model_case` — корень кейса с `model/`).
///
/// # Errors
/// Файл не читается/не парсится, формат не распознан или различается между
/// файлами, модель задана, но не читается.
pub fn diff_report(
    old: &Path,
    new: &Path,
    format_override: Option<ContractFormat>,
    model_case: Option<&Path>,
) -> Result<DiffReport> {
    let old_text = read_text(old)?;
    let new_text = read_text(new)?;
    let format = if let Some(f) = format_override {
        f
    } else {
        let f_old = detect_format(old, &old_text)?;
        let f_new = detect_format(new, &new_text)?;
        if f_old != f_new {
            return Err(HarnessError::Tool(format!(
                "форматы различаются: {} — {}, {} — {} (сравнивать нужно одноформатное)",
                old.display(),
                f_old.name(),
                new.display(),
                f_new.name()
            )));
        }
        f_old
    };
    let mut findings = match format {
        ContractFormat::OpenApi => diff_openapi(&old_text, &new_text, old, new)?,
        ContractFormat::Proto => diff_proto(&old_text, &new_text),
        ContractFormat::Avro => diff_avro(&old_text, &new_text, old, new)?,
        ContractFormat::JsonSchema => diff_jsonschema(&old_text, &new_text, old, new)?,
        ContractFormat::Ddl => diff_ddl(&old_text, &new_text),
    };
    major_rule(format, &old_text, &new_text, &mut findings);
    let impact = match model_case {
        Some(case) => Some(build_impact(case, old, new)?),
        None => None,
    };
    Ok(DiffReport {
        format,
        findings,
        impact,
    })
}

/// Правило «ломающий дифф без смены major — error», где major определим:
/// `OpenAPI` — major-компонент semver `info.version` (CD-007); proto —
/// суффикс `.vN` пакета (CD-P06). Major не определим (нет поля/суффикса) —
/// правило молчит (задокументированное ограничение; у Avro/JSON Schema/DDL
/// версии нет вовсе).
fn major_rule(format: ContractFormat, old: &str, new: &str, out: &mut Vec<Finding>) {
    let breaking = out.iter().filter(|f| f.severity == "error").count();
    if breaking == 0 {
        return;
    }
    let versions: Option<(String, String)> = match format {
        ContractFormat::OpenApi => {
            let old_v = parse_contract(old, Path::new("<old>"))
                .ok()
                .and_then(|d| d.get("info")?.get("version")?.as_str().map(str::to_string));
            let new_v = parse_contract(new, Path::new("<new>"))
                .ok()
                .and_then(|d| d.get("info")?.get("version")?.as_str().map(str::to_string));
            old_v.zip(new_v)
        }
        ContractFormat::Proto => {
            let old_p = parse_proto(old).package;
            let new_p = parse_proto(new).package;
            old_p.zip(new_p)
        }
        _ => None,
    };
    let Some((old_v, new_v)) = versions else {
        return;
    };
    let majors = match format {
        ContractFormat::OpenApi => (semver_major(&old_v), semver_major(&new_v)),
        ContractFormat::Proto => (proto_package_major(&old_v), proto_package_major(&new_v)),
        _ => (None, None),
    };
    let (Some(old_m), Some(new_m)) = majors else {
        return;
    };
    if old_m == new_m {
        let (rule, location, what) = match format {
            ContractFormat::OpenApi => ("CD-007", "#/info/version", "info.version"),
            ContractFormat::Proto => ("CD-P06", "#/proto/package", "major-суффикс пакета (.vN)"),
            _ => unreachable!("major_rule вызывается только для openapi/proto"),
        };
        out.push(Finding {
            severity: "error".into(),
            rule: rule.into(),
            location: location.into(),
            message: format!(
                "ломающих изменений: {breaking}, а {what} не изменился ({old_v} → {new_v}) — \
                 ломающий дифф требует смены major"
            ),
        });
    }
}

/// Major-компонент semver (`1.2.3` → 1).
fn semver_major(version: &str) -> Option<u64> {
    version.trim().split('.').next()?.parse().ok()
}

/// Major proto-пакета: число суффикса `.vN` (`acme.payments.v2` → 2).
fn proto_package_major(package: &str) -> Option<u64> {
    package.rsplit('.').next()?.strip_prefix('v')?.parse().ok()
}

// ---------------------------------------------------------------------------
// Связка с моделью (ADR-035, п.14)
// ---------------------------------------------------------------------------

/// Нормализация пути для сверки с `INT.contract`: относительно корня кейса,
/// без `./` и обратных слэшей.
fn normalize_contract_path(case: &Path, raw: &Path) -> String {
    let stripped = if raw.is_absolute() {
        raw.strip_prefix(case).unwrap_or(raw).to_path_buf()
    } else {
        raw.to_path_buf()
    };
    let mut s = stripped.to_string_lossy().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    s
}

/// Строит связку диффа с моделью: INT, чьё поле `contract` совпало с путём
/// `old`/`new` (относительно корня кейса), и радиус изменения от них
/// ([`crate::review::impact_from_ids`]). Нет совпадений — `matched_int`
/// пуст (gap, а не ошибка: контракт может легитимно жить вне модели).
///
/// # Errors
/// Модель задана, но `model/` не читается/не разбирается.
fn build_impact(case: &Path, old: &Path, new: &Path) -> Result<ContractImpact> {
    let model_dir = case.join("model");
    if !model_dir.is_dir() {
        return Err(HarnessError::Model(format!(
            "contract_diff --model: нет каталога модели {}",
            model_dir.display()
        )));
    }
    let model = load_model(&model_dir)?;
    let old_rel = normalize_contract_path(case, old);
    let new_rel = normalize_contract_path(case, new);
    let mut matched_int: Vec<String> = Vec::new();
    let mut matched_paths: BTreeSet<String> = BTreeSet::new();
    for e in &model.entities {
        if e.kind != EntityKind::Int {
            continue;
        }
        let Some(contract) = e.contract.as_deref().map(str::trim) else {
            continue;
        };
        if contract.is_empty() {
            continue;
        }
        let norm = contract.replace('\\', "/");
        let norm = norm.strip_prefix("./").unwrap_or(&norm).to_string();
        if norm == old_rel || norm == new_rel {
            matched_int.push(e.id.clone());
            matched_paths.insert(norm);
        }
    }
    matched_int.sort();
    if matched_int.is_empty() {
        return Ok(ContractImpact {
            matched_int,
            matched_paths: Vec::new(),
            consumers: Vec::new(),
            rules: Vec::new(),
            owners: Vec::new(),
            summary: String::new(),
        });
    }
    let impact = crate::review::impact_from_ids(case, &matched_int)?;
    let matched: BTreeSet<&str> = matched_int.iter().map(String::as_str).collect();
    let consumers: Vec<String> = impact
        .affected
        .iter()
        .filter(|a| (a.kind == "cmp" || a.kind == "sys") && !matched.contains(a.id.as_str()))
        .map(|a| format!("{} · {}", a.id, a.title))
        .collect();
    let rules: Vec<String> = impact
        .rules
        .iter()
        .map(|r| {
            let name = r.name.as_deref().unwrap_or("?");
            match &r.owner {
                Some(owner) => format!("{} ({name}; владелец: {owner})", r.id),
                None => format!("{} ({name})", r.id),
            }
        })
        .collect();
    Ok(ContractImpact {
        matched_int,
        matched_paths: matched_paths.into_iter().collect(),
        consumers,
        rules,
        owners: impact.owners,
        summary: impact.summary,
    })
}

#[cfg(test)]
mod tests {
    use super::{diff_contracts, diff_report};
    use std::sync::Arc;

    use serde_json::json;

    use crate::contract_diff::testkit::PROTO_V1;
    use crate::contract_diff::tools::tools;
    use crate::tool::ToolContext;

    /// Связка с моделью: INT-001 несёт `contract: contracts/pay.proto`;
    /// ломающий дифф возвращает потребителей (CMP/SYS) и владельцев.
    #[tokio::test]
    async fn model_linkage_returns_consumers_and_owners() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = dir.path().join("case");
        let model = case.join("model");
        std::fs::create_dir_all(&model).expect("mkdir model");
        for (name, fm) in [
            (
                "AD-1.md",
                "---\nid: AD-1\ntype: ad\ntitle: Контракты\nstatus: ADOPTED\nverified_by: [C-001]\n---\n\nПравило.\n",
            ),
            (
                "CMP-001.md",
                "---\nid: CMP-001\ntype: cmp\ntitle: Платёжный шлюз\nstatus: designed\nimplements: [AD-1]\ndepends_on: [INT-001]\n---\n\nТело.\n",
            ),
            (
                "INT-001.md",
                "---\nid: INT-001\ntype: int\ntitle: Рельс процессинга\nstatus: accepted\ncontract: contracts/pay.proto\naffects: [OWNER-1]\n---\n\nТело.\n",
            ),
            (
                "OWNER-1.md",
                "---\nid: OWNER-1\ntype: owner\ntitle: Команда процессинга\nstatus: active\n---\n\nТело.\n",
            ),
        ] {
            std::fs::write(model.join(name), fm).expect("сущность");
        }
        std::fs::write(
            case.join("CONSTRAINTS.yaml"),
            "constraints:\n  - id: C-001\n    name: contract_review\n    owner: Команда платежей\n",
        )
        .expect("constraints");
        let contracts = case.join("contracts");
        std::fs::create_dir_all(&contracts).expect("mkdir contracts");
        std::fs::write(contracts.join("old.proto"), PROTO_V1).expect("old");
        let new_proto = PROTO_V1.replace("  optional string currency = 3;\n", "");
        std::fs::write(contracts.join("new.proto"), new_proto).expect("new");

        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = tools()[0]
            .call(
                json!({
                    "old": "case/contracts/old.proto",
                    "new": "case/contracts/new.proto",
                    "model": "case",
                }),
                &ctx,
            )
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-P02"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // Impact-секция: INT-001 совпал (путь old/new не равен contract
        // строкой — сверка по имени файла? нет: contract=contracts/pay.proto,
        // а дифф по old.proto/new.proto — совпадения НЕТ, gap).
        assert!(out.content.contains("Связь с моделью"), "{}", out.content);
        // Теперь настоящее совпадение: contract указывает на new.proto.
        std::fs::write(
            model.join("INT-001.md"),
            "---\nid: INT-001\ntype: int\ntitle: Рельс процессинга\nstatus: accepted\ncontract: contracts/new.proto\naffects: [OWNER-1]\n---\n\nТело.\n",
        )
        .expect("INT с совпадающим contract");
        let out = tools()[0]
            .call(
                json!({
                    "old": "case/contracts/old.proto",
                    "new": "case/contracts/new.proto",
                    "model": "case",
                }),
                &ctx,
            )
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("INT-001"), "{}", out.content);
        assert!(
            out.content.contains("CMP-001 · Платёжный шлюз"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("OWNER-1 · Команда процессинга"),
            "{}",
            out.content
        );
        assert!(out.content.contains("C-001"), "{}", out.content);
    }

    /// `diff_contracts` возвращает находки ломающего диффа, а не пустой
    /// список «на всякий случай»; идентичные контракты — пусто.
    #[test]
    fn diff_contracts_reports_breaking_change_and_clean_pair() {
        let dir = tempfile::tempdir().expect("tmp");
        let old = dir.path().join("old.proto");
        let new = dir.path().join("new.proto");
        let same = dir.path().join("same.proto");
        std::fs::write(&old, PROTO_V1).expect("old");
        std::fs::write(
            &new,
            PROTO_V1.replace("  optional string currency = 3;\n", ""),
        )
        .expect("new");
        std::fs::write(&same, PROTO_V1).expect("same");
        let findings = diff_contracts(&old, &new).expect("дифф");
        assert!(!findings.is_empty(), "ломающий дифф даёт находки");
        assert!(findings.iter().any(|f| f.rule == "CD-P02"), "{findings:?}");
        assert!(
            diff_contracts(&old, &same).expect("дифф").is_empty(),
            "идентичные контракты — чисто"
        );
    }

    /// Связка с моделью на уровне структуры: INT, чей `contract` совпал с
    /// путём old, попадает в `matched_int`, сам INT в потребители НЕ входит
    /// (он и есть источник), входят только CMP/SYS радиуса.
    #[test]
    fn impact_links_int_by_old_path_and_lists_only_cmp_sys_consumers() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = dir.path().join("case");
        let model = case.join("model");
        std::fs::create_dir_all(&model).expect("mkdir model");
        for (name, fm) in [
            (
                "CMP-001.md",
                "---\nid: CMP-001\ntype: cmp\ntitle: Платёжный шлюз\nstatus: designed\ndepends_on: [INT-001]\n---\n\nТело.\n",
            ),
            (
                "SYS-001.md",
                "---\nid: SYS-001\ntype: sys\ntitle: Процессинг\nstatus: designed\ndepends_on: [INT-001]\n---\n\nТело.\n",
            ),
            (
                "INT-001.md",
                "---\nid: INT-001\ntype: int\ntitle: Рельс процессинга\nstatus: accepted\ncontract: contracts/old.proto\naffects: [OWNER-1]\n---\n\nТело.\n",
            ),
            (
                "INT-002.md",
                "---\nid: INT-002\ntype: int\ntitle: Другой рельс\nstatus: accepted\ncontract: contracts/other.proto\n---\n\nТело.\n",
            ),
            (
                "OWNER-1.md",
                "---\nid: OWNER-1\ntype: owner\ntitle: Команда процессинга\nstatus: active\n---\n\nТело.\n",
            ),
        ] {
            std::fs::write(model.join(name), fm).expect("сущность");
        }
        std::fs::write(case.join("CONSTRAINTS.yaml"), "constraints: []\n").expect("constraints");
        let contracts = case.join("contracts");
        std::fs::create_dir_all(&contracts).expect("mkdir contracts");
        let old = contracts.join("old.proto");
        let new = contracts.join("new.proto");
        std::fs::write(&old, PROTO_V1).expect("old");
        std::fs::write(
            &new,
            PROTO_V1.replace("  optional string currency = 3;\n", ""),
        )
        .expect("new");

        let report = diff_report(&old, &new, None, Some(&case)).expect("дифф с моделью");
        let impact = report.impact.expect("связка с моделью");
        assert_eq!(impact.matched_int, vec!["INT-001".to_string()]);
        assert_eq!(
            impact.matched_paths,
            vec!["contracts/old.proto".to_string()]
        );
        let mut consumers = impact.consumers.clone();
        consumers.sort();
        assert_eq!(
            consumers,
            vec![
                "CMP-001 · Платёжный шлюз".to_string(),
                "SYS-001 · Процессинг".to_string(),
            ],
            "в потребители входят только CMP/SYS: ни сам INT-001, ни владелец OWNER-1 из радиуса"
        );
    }
}
