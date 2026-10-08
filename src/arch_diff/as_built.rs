//! Граф «как построено» (`as_built`): компоненты, внешние системы, хранилища
//! и контракты ревизии + рёбра между ними с основаниями `файл:строка` (K1).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use regex::Regex;

use super::snapshot::{Snapshot, snapshot_at, snapshot_worktree};
use super::types::{ArchEdge, ArchGraph, ArchNode, EdgeKind, MAX_EDGE_EVIDENCE, NodeKind};
use crate::control::{DiffGlobs, content_looks_like_contract, looks_like_config};
use crate::error::{HarnessError, Result};
use crate::imports;
use crate::model::{EntityKind, Model, load_model_tolerant};
use crate::survey::{INTEGRATION_URL_PATTERN, extract_host_port, is_manifest_file};

/// Потолок байт заголовка контракта для опознания по содержимому — как у
/// детектора `api_contract_change` (`CONTRACT_PROBE_BYTES` в `diff_triggers`).
const CONTRACT_PROBE_BYTES: usize = 4096;

/// Схемы строк подключения, относимые к хранилищам (а не к внешним
/// системам): подмножество паттерна детектора `new_datastore`
/// (`DATASTORE_PATTERN` в `control::diff_triggers`) с явной схемой.
const DATASTORE_SCHEMES: [&str; 8] = [
    "postgres",
    "postgresql",
    "mysql",
    "mariadb",
    "mongodb",
    "mongodb+srv",
    "redis",
    "kafka",
];

/// Компонент графа в построении: узел + корни кода.
struct ComponentCtx {
    /// Стабильный id узла (id модели либо `dir:<каталог>`).
    id: String,
    /// Заголовок из модели (для выведенных — сам каталог).
    title: String,
    /// Выведен по манифесту сборки (без модели/`code_roots`).
    inferred: bool,
    /// Id сущности модели (у выведенных — None).
    model_id: Option<String>,
    /// Нормализованные корни кода (пустая строка — корень репозитория).
    roots: Vec<String>,
}

/// Граф «как построено» по ревизии `rev` репозитория `repo` с дефолтными
/// глобами контрактов ([`DiffGlobs::default`]).
///
/// # Errors
/// Как у [`snapshot_at`]; плюс ошибки чтения модели снимка.
pub fn as_built(repo: &Path, rev: &str) -> Result<ArchGraph> {
    as_built_with(repo, rev, &DiffGlobs::default())
}

/// Граф «как построено» с явными глобами контрактов (T-05, `[significance]`
/// конфига). Рабочее дерево не изменяется: вся работа — по снимку ревизии.
///
/// # Errors
/// Как у [`snapshot_at`]; плюс ошибки чтения модели снимка.
pub fn as_built_with(repo: &Path, rev: &str, globs: &DiffGlobs) -> Result<ArchGraph> {
    Ok(scan_revision(repo, rev, globs)?.graph)
}

/// Результат сканирования одной ревизии: снимок файлов, граф as-built и
/// модель (дифф K2 пользуется всеми тремя; `as_built` — только графом).
pub(crate) struct RevisionScan {
    /// Снимок файлов ревизии.
    pub snapshot: Snapshot,
    /// Граф «как построено».
    pub graph: ArchGraph,
    /// Модель ревизии (если в снимке есть `model/*.md`).
    pub model: Option<Model>,
    /// Материализация `model/` и реестра правил ревизии: держит файлы,
    /// пока жива модель (её `file`-пути указывают сюда).
    case: Option<tempfile::TempDir>,
}

impl RevisionScan {
    /// Каталог материализации кейса ревизии (`model/`, `CONSTRAINTS.yaml`) —
    /// вход для NFR-проверок и чтения правил (K2).
    pub(crate) fn case_dir(&self) -> Option<&Path> {
        self.case.as_ref().map(tempfile::TempDir::path)
    }

    /// Путь файла сущности модели относительно корня кейса
    /// (`model/CMP-001-intake.md`) — для предложений правки модели.
    pub(crate) fn model_file_rel(&self, abs: &Path) -> Option<String> {
        let case = self.case.as_ref()?;
        abs.strip_prefix(case.path())
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
    }
}

/// Сканирует ревизию: снимок + материализация `model/` и реестра правил
/// во временный каталог + граф.
pub(crate) fn scan_revision(repo: &Path, rev: &str, globs: &DiffGlobs) -> Result<RevisionScan> {
    let snap = snapshot_at(repo, rev)?;
    let (case, model) = materialize_case(&snap)?;
    let graph = build_graph(&snap, model.as_ref(), globs)?;
    Ok(RevisionScan {
        snapshot: snap,
        graph,
        model,
        case,
    })
}

/// Сканирует РАБОЧЕЕ ДЕРЕВО (K6): то же, что [`scan_revision`], но по снимку
/// файловой системы — видны незакоммиченные правки. Голова диффа составляющей
/// гейта `arch_drift` (git-ревизий свободных не тратим: работа агента ещё не
/// закоммичена).
pub(crate) fn scan_worktree(repo: &Path, globs: &DiffGlobs) -> Result<RevisionScan> {
    let snap = snapshot_worktree(repo)?;
    let (case, model) = materialize_case(&snap)?;
    let graph = build_graph(&snap, model.as_ref(), globs)?;
    Ok(RevisionScan {
        snapshot: snap,
        graph,
        model,
        case,
    })
}

/// Материализует из снимка файлы модели (`model/*.md`) и реестр правил
/// (`CONSTRAINTS.yaml` / `.arch-handoff/CONSTRAINTS.yaml`) во временный
/// каталог-кейс и загружает модель толерантно (E3). Без модели и реестра
/// каталог не создаётся.
fn materialize_case(snap: &Snapshot) -> Result<(Option<tempfile::TempDir>, Option<Model>)> {
    let mut payloads: Vec<(&str, &str)> = Vec::new();
    for (path, content) in &snap.contents {
        let is_model_doc = path.starts_with("model/")
            && Path::new(path)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        let is_constraints = path == "CONSTRAINTS.yaml" || path == ".arch-handoff/CONSTRAINTS.yaml";
        if is_model_doc || is_constraints {
            payloads.push((path, content));
        }
    }
    if payloads.is_empty() {
        return Ok((None, None));
    }
    let tmp = tempfile::tempdir().map_err(|e| HarnessError::io(Path::new("arch-diff"), e))?;
    let mut has_model = false;
    for (path, content) in payloads {
        has_model |= path.starts_with("model/");
        let target = tmp.path().join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(&target, content).map_err(|e| HarnessError::io(&target, e))?;
    }
    let model = if has_model {
        Some(load_model_tolerant(&tmp.path().join("model"))?)
    } else {
        None
    };
    Ok((Some(tmp), model))
}

/// Собирает компоненты графа: CMP модели с `code_roots` плюс каталоги с
/// манифестом сборки, не покрытые ни одним корнем (пометка `inferred` —
/// тот же набор манифестов, что у `survey`/дрейфа модели).
fn collect_components(snap: &Snapshot, model: Option<&Model>) -> Vec<ComponentCtx> {
    let mut comps: Vec<ComponentCtx> = Vec::new();
    if let Some(model) = model {
        for e in &model.entities {
            if e.kind != EntityKind::Cmp || e.code_roots.is_empty() {
                continue;
            }
            let roots: Vec<String> = e
                .code_roots
                .iter()
                .map(|r| imports::normalize_root(r))
                .map(|r| if r == "." { String::new() } else { r })
                .collect();
            comps.push(ComponentCtx {
                id: e.id.clone(),
                title: e.title.clone(),
                inferred: false,
                model_id: Some(e.id.clone()),
                roots,
            });
        }
    }
    // Каталоги с манифестом сборки без покрывающего code_roots — компоненты
    // «по факту» (inferred): граф работает и без модели.
    let mut manifest_dirs: BTreeSet<String> = BTreeSet::new();
    for path in &snap.files {
        let name = path.rsplit('/').next().unwrap_or(path.as_str());
        if is_manifest_file(name) {
            let dir = path
                .rsplit_once('/')
                .map_or(String::new(), |(d, _)| d.to_string());
            manifest_dirs.insert(dir);
        }
    }
    for dir in manifest_dirs {
        let covered = comps.iter().any(|c| {
            c.roots
                .iter()
                .any(|r| r.is_empty() || imports::module_prefix_match(&dir, r))
        });
        if !covered {
            comps.push(ComponentCtx {
                id: format!("dir:{}", if dir.is_empty() { "." } else { &dir }),
                title: if dir.is_empty() {
                    "(корень репозитория)".to_string()
                } else {
                    dir.clone()
                },
                inferred: true,
                model_id: None,
                roots: vec![dir],
            });
        }
    }
    comps.sort_by(|a, b| a.id.cmp(&b.id));
    comps
}

/// Контексты в форме `imports`: (индекс компонента, корни).
fn contexts_of(comps: &[ComponentCtx]) -> Vec<(usize, Vec<String>)> {
    comps
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.roots.clone()))
        .collect()
}

/// Владелец пути среди компонентов: точный префикс корня; пустой корень
/// (манифест в корне репозитория) — запасной владелец «всего дерева»,
/// чтобы корневой код однокомпонентного репозитория не терялся.
fn owner_of_path(contexts: &[(usize, Vec<String>)], path: &str) -> Option<usize> {
    if let Some(hit) = imports::owner_by_path(contexts, path) {
        return Some(*hit);
    }
    contexts
        .iter()
        .find(|(_, roots)| roots.iter().any(String::is_empty))
        .map(|(c, _)| *c)
}

/// Накопитель рёбер: (from, to, kind) → основания (множество — дедуп).
type EdgeAcc = BTreeMap<(String, String, EdgeKind), BTreeSet<String>>;

/// Добавляет основание к ребру (создавая его).
fn acc_edge(acc: &mut EdgeAcc, from: &str, to: &str, kind: EdgeKind, evidence: String) {
    acc.entry((from.to_string(), to.to_string(), kind))
        .or_default()
        .insert(evidence);
}

/// Рёбра импортов: файл компонента импортирует модуль другого компонента.
fn collect_import_edges(snap: &Snapshot, comps: &[ComponentCtx], acc: &mut EdgeAcc) -> Result<()> {
    let contexts = contexts_of(comps);
    let bases = imports::candidate_bases_with_crates(imports::crate_dirs_from_paths(
        snap.files.iter().map(String::as_str),
    ));
    let exists = |p: &str| snap.exists(p);
    for (path, content) in &snap.contents {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if !imports::IMPORT_EXTENSIONS.contains(&ext) {
            continue;
        }
        let Some(source) = owner_of_path(&contexts, path) else {
            continue;
        };
        for edge in imports::extract_imports(path, content)? {
            // Прямое разрешение (префикс корня, файл репозитория, относительный
            // путь), затем суффиксное (Go/монорепо-пакеты: импорт оканчивается
            // на корень цели). Неразрешённое — внешняя зависимость, не ребро.
            let target =
                imports::resolve_import_owner(&contexts, &bases, path, &edge.module, &exists)
                    .or_else(|| imports::owner_by_suffix(&contexts, &edge.module));
            let Some(target) = target.copied() else {
                continue;
            };
            if target == source {
                continue;
            }
            acc_edge(
                acc,
                &comps[source].id,
                &comps[target].id,
                EdgeKind::Import,
                format!("{path}:{}", edge.line),
            );
        }
    }
    Ok(())
}

/// Классификация URL из конфига: хранилище (`store:<scheme>://<host>`) или
/// внешняя система (`sys:<host:port>`); петлевые хосты пропускаются (как в
/// `survey`). userinfo срезается разбором `survey::extract_host_port`;
/// итоговый id дополнительно прогоняется через редактор секретов.
fn classify_url(url: &str, redactor: &crate::secrets::Redactor) -> Option<(NodeKind, String)> {
    let host = extract_host_port(url)?;
    let scheme_raw = url
        .split_once("://")
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_default();
    // jdbc:postgresql://… — хранилище по подсхеме.
    let scheme = scheme_raw.strip_prefix("jdbc:").unwrap_or(&scheme_raw);
    let (kind, id) = if DATASTORE_SCHEMES.contains(&scheme) {
        (NodeKind::Datastore, format!("store:{scheme}://{host}"))
    } else {
        (NodeKind::ExternalSystem, format!("sys:{host}"))
    };
    Some((kind, redactor.redact(&id)))
}

/// Рёбра обращений: строки подключения в конфигах компонентов → внешние
/// системы (`sys:`) и хранилища (`store:`); узлы создаются и тогда, когда
/// конфиг не принадлежит ни одному компоненту (ребра тогда нет).
fn collect_connect_edges(
    snap: &Snapshot,
    comps: &[ComponentCtx],
    acc: &mut EdgeAcc,
    nodes: &mut BTreeMap<String, ArchNode>,
) -> Result<()> {
    let contexts = contexts_of(comps);
    let re_url = Regex::new(INTEGRATION_URL_PATTERN)
        .map_err(|e| HarnessError::Control(format!("внутренний regex интеграций: {e}")))?;
    let redactor = crate::secrets::Redactor::with_builtin_rules();
    for (path, content) in &snap.contents {
        if !looks_like_config(path) {
            continue;
        }
        let owner = owner_of_path(&contexts, path);
        for (idx, line) in content.lines().enumerate() {
            let mut hits: Vec<(NodeKind, String)> = Vec::new();
            for m in re_url.find_iter(line) {
                if let Some(hit) = classify_url(m.as_str(), &redactor) {
                    hits.push(hit);
                }
            }
            // `bootstrap.servers: "k1:9092,k2:9092"` — Kafka без схемы (та же
            // альтернатива, что у детектора `new_datastore`): значение
            // целиком (кавычки срезаются), каждый брокер — свой узел.
            if let Some((_, rest)) = line.split_once("bootstrap.servers") {
                let rest = rest.trim_start_matches([' ', ':', '=']);
                for token in rest.split(',') {
                    let token = token.trim().trim_matches(['"', '\'']);
                    if token.is_empty() {
                        continue;
                    }
                    if let Some(host) = extract_host_port(token) {
                        hits.push((
                            NodeKind::Datastore,
                            redactor.redact(&format!("store:kafka://{host}")),
                        ));
                    }
                }
            }
            for (kind, id) in hits {
                nodes.entry(id.clone()).or_insert_with(|| ArchNode {
                    title: id.clone(),
                    id: id.clone(),
                    kind,
                    inferred: false,
                    model_id: None,
                });
                if let Some(source) = owner {
                    acc_edge(
                        acc,
                        &comps[source].id,
                        &id,
                        EdgeKind::Connect,
                        format!("{path}:{}", idx + 1),
                    );
                }
            }
        }
    }
    Ok(())
}

/// Контракт ли путь: имя (openapi/asyncapi), глоб контрактов (T-05),
/// расширение `.proto` либо ключ верхнего уровня в заголовке содержимого —
/// тот же набор эвристик, что у детектора `api_contract_change`.
fn is_contract_path(snap: &Snapshot, globs: &DiffGlobs, path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if name.contains("openapi") || name.contains("asyncapi") {
        return true;
    }
    if globs
        .contracts
        .iter()
        .any(|g| crate::control::glob_matches(g, path))
    {
        return true;
    }
    if Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("proto"))
    {
        return true;
    }
    snap.content(path).is_some_and(|content| {
        let head = content.get(..CONTRACT_PROBE_BYTES).unwrap_or(content);
        content_looks_like_contract(head)
    })
}

/// Все контрактные пути снимка (дифф K2 сравнивает их между ревизиями).
pub(crate) fn contract_paths(snap: &Snapshot, globs: &DiffGlobs) -> BTreeSet<String> {
    snap.files
        .iter()
        .filter(|p| is_contract_path(snap, globs, p))
        .cloned()
        .collect()
}

/// Узлы контрактов и рёбра «компонент реализует/публикует контракт» (файл
/// контракта внутри корня компонента).
fn collect_contracts(
    snap: &Snapshot,
    comps: &[ComponentCtx],
    globs: &DiffGlobs,
    acc: &mut EdgeAcc,
    nodes: &mut BTreeMap<String, ArchNode>,
) {
    let contexts = contexts_of(comps);
    for path in &snap.files {
        if !is_contract_path(snap, globs, path) {
            continue;
        }
        let id = format!("contract:{path}");
        nodes.entry(id.clone()).or_insert_with(|| ArchNode {
            title: path.clone(),
            id: id.clone(),
            kind: NodeKind::Contract,
            inferred: false,
            model_id: None,
        });
        if let Some(source) = owner_of_path(&contexts, path) {
            acc_edge(
                acc,
                &comps[source].id,
                &id,
                EdgeKind::ContractRef,
                format!("{path}:1"),
            );
        }
    }
}

/// Сборка графа из снимка и модели: узлы отсортированы по id, рёбра — по
/// (from, to, kind), основания — по строке с потолком [`MAX_EDGE_EVIDENCE`].
fn build_graph(snap: &Snapshot, model: Option<&Model>, globs: &DiffGlobs) -> Result<ArchGraph> {
    let comps = collect_components(snap, model);
    let mut nodes: BTreeMap<String, ArchNode> = BTreeMap::new();
    for c in &comps {
        nodes.insert(
            c.id.clone(),
            ArchNode {
                id: c.id.clone(),
                kind: NodeKind::Component,
                title: c.title.clone(),
                inferred: c.inferred,
                model_id: c.model_id.clone(),
            },
        );
    }
    let mut acc: EdgeAcc = BTreeMap::new();
    collect_import_edges(snap, &comps, &mut acc)?;
    collect_connect_edges(snap, &comps, &mut acc, &mut nodes)?;
    collect_contracts(snap, &comps, globs, &mut acc, &mut nodes);

    let mut edges: Vec<ArchEdge> = acc
        .into_iter()
        .map(|((from, to, kind), evidence)| {
            let total = evidence.len();
            let mut ev: Vec<String> = evidence.into_iter().take(MAX_EDGE_EVIDENCE).collect();
            if total > MAX_EDGE_EVIDENCE {
                ev.push(format!("… (+{} ещё)", total - MAX_EDGE_EVIDENCE));
            }
            ArchEdge {
                from,
                to,
                kind,
                evidence: ev,
            }
        })
        .collect();
    edges.sort_by(|a, b| (&a.from, &a.to, a.kind).cmp(&(&b.from, &b.to, b.kind)));
    Ok(ArchGraph {
        rev: snap.rev.clone(),
        nodes: nodes.into_values().collect(),
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::super::snapshot::tests::{git_repo, write_file};
    use super::*;
    use crate::arch_diff::NodeKind as NK;

    /// Модель из двух CMP: приём (`skeleton/intake`) и ядро (`skeleton/ledger`).
    fn write_model(repo: &Path) {
        write_file(
            repo,
            "model/CMP-001-intake.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём реестров\nstatus: adopted\ncode_roots: [skeleton/intake]\n---\nПриём.\n",
        );
        write_file(
            repo,
            "model/CMP-004-ledger.md",
            "---\nid: CMP-004\ntype: cmp\ntitle: \"Ядро: счета и проводки\"\nstatus: adopted\ncode_roots: [skeleton/ledger]\n---\nЯдро.\n",
        );
    }

    /// Python-фикстура демо-сценария: приём импортирует ядро напрямую.
    fn fixture_python(repo: &Path) {
        write_model(repo);
        write_file(
            repo,
            "skeleton/intake/writer.py",
            "from skeleton.ledger import client\n\ndef write():\n    client.post()\n",
        );
        write_file(repo, "skeleton/ledger/client.py", "def post():\n    pass\n");
        write_file(
            repo,
            "skeleton/intake/pyproject.toml",
            "[project]\nname = \"intake\"\n",
        );
    }

    #[test]
    fn python_import_edge_with_evidence() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_python(&repo);
        git_repo(&repo);

        let graph = as_built(&repo, "HEAD").expect("граф");
        assert_eq!(graph.count_kind(NK::Component), 2, "{graph:?}");
        let edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Import)
            .expect("ребро импорта");
        assert_eq!(edge.from, "CMP-001");
        assert_eq!(edge.to, "CMP-004");
        assert_eq!(edge.evidence, vec!["skeleton/intake/writer.py:1"]);
        // Узлы — из модели (не inferred).
        assert!(graph.nodes.iter().all(|n| !n.inferred));
    }

    /// Rust: `crate::…` разрешается в файл под базой `src/`, владелец — по
    /// префиксу корня.
    #[test]
    fn rust_crate_path_edge() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "model/CMP-001-intake.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём\nstatus: adopted\ncode_roots: [src/intake]\n---\n",
        );
        write_file(
            &repo,
            "model/CMP-002-ledger.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Ядро\nstatus: adopted\ncode_roots: [src/ledger]\n---\n",
        );
        write_file(&repo, "Cargo.toml", "[package]\nname = \"bank\"\n");
        write_file(
            &repo,
            "src/intake/writer.rs",
            "use crate::ledger::postings::post;\n",
        );
        write_file(&repo, "src/ledger/postings.rs", "pub fn post() {}\n");
        git_repo(&repo);

        let graph = as_built(&repo, "HEAD").expect("граф");
        let edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Import)
            .expect("ребро");
        assert_eq!(
            (edge.from.as_str(), edge.to.as_str()),
            ("CMP-001", "CMP-002")
        );
        assert_eq!(edge.evidence, vec!["src/intake/writer.rs:1"]);
    }

    /// Java: пакет `ru.bank.ledger` разрешается в файл под базой
    /// `src/main/java/`, владелец — по префиксу корня пакета.
    #[test]
    fn java_package_edge() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "model/CMP-001-intake.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём\nstatus: adopted\ncode_roots: [src/main/java/ru/bank/intake]\n---\n",
        );
        write_file(
            &repo,
            "model/CMP-002-ledger.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Ядро\nstatus: adopted\ncode_roots: [src/main/java/ru/bank/ledger]\n---\n",
        );
        write_file(&repo, "pom.xml", "<project/>\n");
        write_file(
            &repo,
            "src/main/java/ru/bank/intake/Intake.java",
            "package ru.bank.intake;\nimport ru.bank.ledger.Core;\n",
        );
        write_file(
            &repo,
            "src/main/java/ru/bank/ledger/Core.java",
            "package ru.bank.ledger;\npublic class Core {}\n",
        );
        git_repo(&repo);

        let graph = as_built(&repo, "HEAD").expect("граф");
        let edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Import)
            .expect("ребро");
        assert_eq!(
            (edge.from.as_str(), edge.to.as_str()),
            ("CMP-001", "CMP-002")
        );
    }

    /// Go: импорт `example.com/bank/ledger` разрешается суффиксным
    /// совпадением с корнем `ledger` (монорепо-пакеты).
    #[test]
    fn go_suffix_resolution_edge() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "model/CMP-001-intake.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём\nstatus: adopted\ncode_roots: [intake]\n---\n",
        );
        write_file(
            &repo,
            "model/CMP-002-ledger.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Ядро\nstatus: adopted\ncode_roots: [ledger]\n---\n",
        );
        write_file(&repo, "go.mod", "module example.com/bank\n\ngo 1.24\n");
        write_file(
            &repo,
            "intake/main.go",
            "package main\n\nimport (\n\t\"fmt\"\n\t\"example.com/bank/ledger\"\n)\n\nfunc main() { fmt.Println(ledger.X) }\n",
        );
        write_file(&repo, "ledger/ledger.go", "package ledger\n\nvar X = 1\n");
        git_repo(&repo);

        let graph = as_built(&repo, "HEAD").expect("граф");
        let edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Import)
            .expect("ребро");
        assert_eq!(
            (edge.from.as_str(), edge.to.as_str()),
            ("CMP-001", "CMP-002")
        );
        assert_eq!(edge.evidence, vec!["intake/main.go:5"]);
    }

    /// Без модели: компоненты выводятся по манифестам сборки с пометкой
    /// `inferred`, ребра — по префиксу корней.
    #[test]
    fn inferred_graph_without_model() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "intake/pyproject.toml",
            "[project]\nname = \"intake\"\n",
        );
        write_file(&repo, "intake/app.py", "import ledger.core\n");
        write_file(
            &repo,
            "ledger/pyproject.toml",
            "[project]\nname = \"ledger\"\n",
        );
        write_file(&repo, "ledger/core.py", "X = 1\n");
        git_repo(&repo);

        let graph = as_built(&repo, "HEAD").expect("граф");
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["dir:intake", "dir:ledger"], "{ids:?}");
        assert!(graph.nodes.iter().all(|n| n.inferred));
        let edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Import)
            .expect("ребро");
        assert_eq!(
            (edge.from.as_str(), edge.to.as_str()),
            ("dir:intake", "dir:ledger")
        );
    }

    /// Внешние системы (`sys:`) и хранилища (`store:`) из конфигов: схема
    /// классифицирует узел, userinfo срезается, петлевые пропускаются.
    #[test]
    fn external_system_and_datastore_nodes() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_python(&repo);
        write_file(
            &repo,
            "skeleton/intake/config.yaml",
            "upstream: \"https://api.bank.ru:8443/v1\"\ndsn: \"postgres://user:secret@ledger-db:5432/ledger\"\nlocal: \"http://127.0.0.1:9000/x\"\nqueue: \"amqp://mq.internal:5672\"\nbootstrap.servers: \"kafka-1:9092,kafka-2:9092\"\n",
        );
        git_repo(&repo);
        let graph = as_built(&repo, "HEAD").expect("граф");
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"sys:api.bank.ru:8443"), "{ids:?}");
        assert!(ids.contains(&"sys:mq.internal:5672"), "{ids:?}");
        assert!(ids.contains(&"store:postgres://ledger-db:5432"), "{ids:?}");
        assert!(ids.contains(&"store:kafka://kafka-1:9092"), "{ids:?}");
        assert!(ids.contains(&"store:kafka://kafka-2:9092"), "{ids:?}");
        assert!(!ids.iter().any(|i| i.contains("127.0.0.1")), "{ids:?}");
        assert!(!ids.iter().any(|i| i.contains("secret")), "{ids:?}");
        let pg = graph
            .edges
            .iter()
            .find(|e| e.to == "store:postgres://ledger-db:5432")
            .expect("ребро к хранилищу");
        assert_eq!(pg.from, "CMP-001");
        assert_eq!(pg.kind, EdgeKind::Connect);
        assert_eq!(pg.evidence, vec!["skeleton/intake/config.yaml:2"]);
    }

    /// Контракты: по содержимому (openapi-ключ), по имени и по `.proto`;
    /// файл внутри корня компонента даёт ребро contract.
    #[test]
    fn contract_nodes_and_edges() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_python(&repo);
        write_file(
            &repo,
            "skeleton/intake/api.yaml",
            "openapi: 3.0.3\ninfo: {title: Intake, version: 1.0.0}\npaths: {}\n",
        );
        write_file(&repo, "contracts/asyncapi-push.yaml", "asyncapi: 3.0.0\n");
        write_file(
            &repo,
            "proto/ledger/v1/ledger.proto",
            "syntax = \"proto3\";\n",
        );
        git_repo(&repo);

        let graph = as_built(&repo, "HEAD").expect("граф");
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.contains(&"contract:skeleton/intake/api.yaml"),
            "{ids:?}"
        );
        assert!(
            ids.contains(&"contract:contracts/asyncapi-push.yaml"),
            "{ids:?}"
        );
        assert!(
            ids.contains(&"contract:proto/ledger/v1/ledger.proto"),
            "{ids:?}"
        );
        let edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::ContractRef)
            .expect("ребро контракта");
        assert_eq!(edge.from, "CMP-001");
        assert_eq!(edge.to, "contract:skeleton/intake/api.yaml");
    }

    /// Детерминизм (правило 10): два прогона по одной ревизии дают
    /// байт-в-байт тот же JSON.
    #[test]
    fn as_built_is_deterministic() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_python(&repo);
        write_file(
            &repo,
            "skeleton/intake/config.yaml",
            "dsn: \"postgres://ledger-db:5432/ledger\"\n",
        );
        write_file(
            &repo,
            "skeleton/intake/api.yaml",
            "openapi: 3.0.3\npaths: {}\n",
        );
        git_repo(&repo);

        let a = as_built(&repo, "HEAD").expect("граф");
        let b = as_built(&repo, "HEAD").expect("граф");
        let ja = serde_json::to_string_pretty(&a).expect("json");
        let jb = serde_json::to_string_pretty(&b).expect("json");
        assert_eq!(ja, jb, "повторный прогон обязан совпадать байт-в-байт");
        // Узлы и рёбра отсортированы.
        let ids: Vec<&str> = a.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }
}
