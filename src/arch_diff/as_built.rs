//! Граф «как построено» (`as_built`): компоненты, внешние системы, хранилища
//! и контракты ревизии + рёбра между ними с основаниями `файл:строка` (K1).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use regex::Regex;

use super::snapshot::{Snapshot, snapshot_at_with, snapshot_worktree_with};
use super::types::{
    ArchEdge, ArchGraph, ArchNode, Condition, ConditionKind, Confidence, EdgeKind,
    MAX_EDGE_EVIDENCE, NodeKind, Via,
};
use crate::control::{DiffGlobs, content_looks_like_contract, looks_like_config};
use crate::error::{HarnessError, Result};
use crate::imports;
use crate::model::{EntityKind, Model, load_model_tolerant};
use crate::survey::{INTEGRATION_URL_PATTERN, IntegrationHit, extract_host_port, is_manifest_file};

/// Потолок байт заголовка контракта для опознания по содержимому — как у
/// детектора `api_contract_change` (`CONTRACT_PROBE_BYTES` в `diff_triggers`).
const CONTRACT_PROBE_BYTES: usize = 4096;

/// Схемы строк подключения, относимые к хранилищам (а не к внешним
/// системам): подмножество паттерна детектора `new_datastore`
/// (`DATASTORE_PATTERN` в `control::diff_triggers`) с явной схемой.
pub(crate) const DATASTORE_SCHEMES: [&str; 8] = [
    "postgres",
    "postgresql",
    "mysql",
    "mariadb",
    "mongodb",
    "mongodb+srv",
    "redis",
    "kafka",
];

/// Regex `host:port` без схемы в манифесте (R3): имя хоста начинается с буквы,
/// порт — 2–5 цифр сразу после `:` (`port: 8080` с пробелом не подходит).
const MANIFEST_HOST_PORT_PATTERN: &str = r"(?i)\b([a-z][a-z0-9._-]*):([0-9]{2,5})\b";

/// Компонент графа в построении: узел + корни кода.
pub(crate) struct ComponentCtx {
    /// Стабильный id узла (id модели либо `dir:<каталог>`).
    pub(crate) id: String,
    /// Заголовок из модели (для выведенных — сам каталог).
    pub(crate) title: String,
    /// Выведен по манифесту сборки (без модели/`code_roots`).
    pub(crate) inferred: bool,
    /// Id сущности модели (у выведенных — None).
    pub(crate) model_id: Option<String>,
    /// Нормализованные корни кода (пустая строка — корень репозитория).
    pub(crate) roots: Vec<String>,
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
    /// Компоненты графа с корнями кода (R5: черновик `model/`, детекторы
    /// секций 1/4/5 карты).
    pub components: Vec<ComponentCtx>,
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
    scan_revision_with(repo, rev, globs, &super::snapshot::ScanLimits::default())
}

/// [`scan_revision`] с явными пределами снимка (`ignore`/`max_files` из
/// `[gate.arch_drift]`, ADR-068 Am.2). База и голова диффа обязаны
/// сканироваться с ОДНИМИ пределами — иначе исключённый путь «исчезает» на
/// одной стороне и рождает ложные рёбра.
pub(crate) fn scan_revision_with(
    repo: &Path,
    rev: &str,
    globs: &DiffGlobs,
    limits: &super::snapshot::ScanLimits,
) -> Result<RevisionScan> {
    let snap = snapshot_at_with(repo, rev, limits)?;
    let (case, model) = materialize_case(&snap)?;
    let (graph, components) = build_graph(&snap, model.as_ref(), globs)?;
    Ok(RevisionScan {
        snapshot: snap,
        graph,
        components,
        model,
        case,
    })
}

/// Сканирует РАБОЧЕЕ ДЕРЕВО (K6): то же, что [`scan_revision`], но по снимку
/// файловой системы — видны незакоммиченные правки. Голова диффа составляющей
/// гейта `arch_drift` (git-ревизий свободных не тратим: работа агента ещё не
/// закоммичена). `limits` — пределы снимка (`ignore`/`max_files` из
/// `[gate.arch_drift]`, ADR-068 Am.2).
pub(crate) fn scan_worktree_with(
    repo: &Path,
    globs: &DiffGlobs,
    limits: &super::snapshot::ScanLimits,
) -> Result<RevisionScan> {
    let snap = snapshot_worktree_with(repo, limits)?;
    let (case, model) = materialize_case(&snap)?;
    let (graph, components) = build_graph(&snap, model.as_ref(), globs)?;
    Ok(RevisionScan {
        snapshot: snap,
        graph,
        components,
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

/// Собирает компоненты графа: CMP модели с `code_roots` плюс каталоги-
/// кандидаты без покрывающего корня (пометка `inferred`).
///
/// R1: кандидаты — каталоги с манифестом сборки и модули, объявленные
/// агрегатором (`<modules>`/`include`/`go.work`/`workspaces`/`.sln`); сам
/// агрегирующий корень и каталоги тестовых проектов компонентами НЕ являются.
/// Заголовок выведенного компонента — имя юнита развёртывания (сопряжение
/// модуля сборки и deploy-юнита, [`super::modules::component_name`]), иначе
/// имя каталога.
fn collect_components(
    snap: &Snapshot,
    model: Option<&Model>,
    topo: &super::modules::DeployTopology,
) -> Vec<ComponentCtx> {
    let layout = super::modules::build_layout(snap);
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
    let mut candidate_dirs: BTreeSet<String> = BTreeSet::new();
    for path in &snap.files {
        let name = path.rsplit('/').next().unwrap_or(path.as_str());
        if is_manifest_file(name) {
            let dir = path
                .rsplit_once('/')
                .map_or(String::new(), |(d, _)| d.to_string());
            candidate_dirs.insert(dir);
        }
    }
    // Объявленные агрегатором модули, реально существующие в ревизии
    // (каталог модуля может не иметь манифеста в наборе `is_manifest_file`).
    for m in &layout.modules {
        if super::modules::dir_present(snap, m) {
            candidate_dirs.insert(m.clone());
        }
    }
    candidate_dirs.retain(|dir| !layout.is_aggregator(dir) && !layout.is_test(dir));
    // R4: одномодульный репозиторий (единственный модуль сборки = корень, модели
    // нет) — компонент «по факту» разбирается на кластеры пакетов, иначе он
    // выродился бы в корень репозитория. Пусто — кластеризовать нечего.
    if layout.modules.is_empty() && comps.is_empty() && candidate_dirs.iter().all(String::is_empty)
    {
        let clusters = super::packages::clusters(snap);
        if clusters.len() >= 2 {
            return clusters
                .into_iter()
                .map(|c| ComponentCtx {
                    id: c.id,
                    title: c.title,
                    inferred: true,
                    model_id: None,
                    roots: c.roots,
                })
                .collect();
        }
    }
    for dir in candidate_dirs {
        let covered = comps.iter().any(|c| {
            c.roots
                .iter()
                .any(|r| r.is_empty() || imports::module_prefix_match(&dir, r))
        });
        if !covered {
            comps.push(ComponentCtx {
                id: format!("dir:{}", if dir.is_empty() { "." } else { &dir }),
                title: super::modules::component_name(&dir, topo),
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
pub(crate) fn contexts_of(comps: &[ComponentCtx]) -> Vec<(usize, Vec<String>)> {
    comps
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.roots.clone()))
        .collect()
}

/// Владелец пути среди компонентов: точный префикс корня; пустой корень
/// (манифест в корне репозитория) — запасной владелец «всего дерева»,
/// чтобы корневой код однокомпонентного репозитория не терялся.
pub(crate) fn owner_of_path(contexts: &[(usize, Vec<String>)], path: &str) -> Option<usize> {
    if let Some(hit) = imports::owner_by_path(contexts, path) {
        return Some(*hit);
    }
    contexts
        .iter()
        .find(|(_, roots)| roots.iter().any(String::is_empty))
        .map(|(c, _)| *c)
}

/// Ключ накопителя рёбер: (from, to, kind, via, confidence, condition).
/// `via`/`confidence` заполнены только у рёбер `calls` (R2); `condition` — у
/// рёбер `connect`, выведённых из условного основания (REC, ADR-075); у прочих
/// видов поля `None`.
type EdgeKey = (
    String,
    String,
    EdgeKind,
    Option<Via>,
    Option<Confidence>,
    Option<Condition>,
);

/// Накопитель рёбер: ключ → основания (множество — дедуп).
pub(crate) type EdgeAcc = BTreeMap<EdgeKey, BTreeSet<String>>;

/// Добавляет основание к ребру без способа/достоверности/условия (import/
/// contract по построению).
fn acc_edge(acc: &mut EdgeAcc, from: &str, to: &str, kind: EdgeKind, evidence: String) {
    acc_edge_cond(acc, from, to, kind, None, evidence);
}

/// Добавляет основание к ребру с условием активации (REC, ADR-075): рёбра
/// `connect` одного направления под разными условиями не сливаются — это и
/// есть deployment-вид (одно ребро на окружение).
fn acc_edge_cond(
    acc: &mut EdgeAcc,
    from: &str,
    to: &str,
    kind: EdgeKind,
    condition: Option<Condition>,
    evidence: String,
) {
    acc.entry((
        from.to_string(),
        to.to_string(),
        kind,
        None,
        None,
        condition,
    ))
    .or_default()
    .insert(evidence);
}

/// Токены окружений (нижний регистр) для распознавания overlay-файлов и
/// каталогов. Узкий allowlist — расширение в пользовательском каталоге
/// провайдеров невозможно, поэтому список фиксирован.
const ENV_TOKENS: [&str; 15] = [
    "prod",
    "production",
    "staging",
    "stage",
    "dev",
    "development",
    "test",
    "testing",
    "qa",
    "uat",
    "preprod",
    "sandbox",
    "local",
    "canary",
    "demo",
];

/// Канонический токен окружения из allowlist (нижний регистр) либо `None`.
fn env_token(segment: &str) -> Option<&'static str> {
    let seg = segment.to_ascii_lowercase();
    ENV_TOKENS.iter().copied().find(|t| *t == seg)
}

/// Окружение per-environment overlay-файла (REC, ADR-075) по пути:
/// (`overlays`|`envs`|`environments`)/`<env>`/… (kustomize/helm),
/// `values-<env>.yaml` / `application-<env>.yml` (helm/spring-профили).
/// Эвристика по пути: значение per-env, а не свойство кода.
fn overlay_env(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    let segs: Vec<&str> = lower.split('/').collect();
    // Каталог-маркер окружения: overlays/<env>/, envs/<env>/, environments/<env>/.
    for pair in segs.windows(2) {
        if matches!(pair[0], "overlays" | "envs" | "environments") {
            if let Some(env) = env_token(pair[1]) {
                return Some(env);
            }
        }
    }
    // Имя файла: values-<env>, application-<env>, values_<env> (суффикс — токен).
    let name = segs.last().copied().unwrap_or("");
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    for sep in ['-', '_'] {
        if let Some((_, env)) = stem.rsplit_once(sep) {
            if let Some(token) = env_token(env) {
                return Some(token);
            }
        }
    }
    None
}

/// Условие активации ребра по основанию: overlay-файл перекрывает построчное
/// основание (deployment-вид важнее источника строки), иначе — условие самой
/// строки (config-key/env-flag) либо `None` (безусловное ребро).
fn hit_condition(overlay: Option<&str>, source: Option<Condition>) -> Option<Condition> {
    if let Some(env) = overlay {
        return Some(Condition {
            kind: ConditionKind::EnvOverlay,
            text: env.to_string(),
        });
    }
    source
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

/// Классификация URL из конфига: только хранилище (`store:<scheme>://<host>`)
/// по явной схеме строки подключения. Внешние системы детектируются не по
/// произвольному `host:port` (шум: B-метрика «внешние precision» падала до
/// ≤0.21), а по каталогу провайдеров ([`super::providers`]). Обнаружение
/// интеграции и разбор `host:port` (срез userinfo) — общая механика карты и
/// графа ([`crate::survey::integration_hits`], ADR-069 п.4); денилист B8
/// (`SurveyDenylist`) применяет вызывающий. Итоговый id прогоняется через
/// редактор секретов.
fn classify_url(
    hit: &IntegrationHit,
    redactor: &crate::secrets::Redactor,
) -> Option<(NodeKind, String)> {
    let scheme_raw = hit
        .url
        .split_once("://")
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_default();
    // jdbc:postgresql://… — хранилище по подсхеме.
    let scheme = scheme_raw.strip_prefix("jdbc:").unwrap_or(&scheme_raw);
    if !DATASTORE_SCHEMES.contains(&scheme) {
        return None;
    }
    let id = redactor.redact(&format!("store:{scheme}://{}", hit.host_port));
    Some((NodeKind::Datastore, id))
}

/// Схема хранилища по образу в том же манифесте: `image: redis:alpine` →
/// `redis`. Сравнивается последний компонент имени образа без тега/реестра.
/// Служит разрешением на разбор строк `host:port` без схемы в этом манифесте.
fn storage_image_scheme(content: &str) -> Option<&'static str> {
    for line in content.lines() {
        let Some((_, rest)) = line.split_once("image:") else {
            continue;
        };
        let token = rest
            .trim()
            .trim_matches(['"', '\''])
            .split_whitespace()
            .next()
            .unwrap_or("");
        let name = token.rsplit('/').next().unwrap_or(token);
        let name = name.split(':').next().unwrap_or(name);
        if let Some(scheme) = DATASTORE_SCHEMES
            .iter()
            .copied()
            .find(|s| s.eq_ignore_ascii_case(name))
        {
            return Some(scheme);
        }
    }
    None
}

/// Строка-комментарий (`#`, `//`) — интеграции в ней не ищем.
fn is_comment_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("//")
}

/// Заголовок узла обращения: внешняя система — id провайдера без префикса
/// `sys:` (эталон сверяется по имени провайдера, а не по `sys:<id>`), узлы
/// хранилищ — стабильный id как есть.
fn node_title(kind: NodeKind, id: &str) -> String {
    match kind {
        NodeKind::ExternalSystem => id.strip_prefix("sys:").unwrap_or(id).to_string(),
        _ => id.to_string(),
    }
}

/// Владельцы k8s-документов файла: `(строка начала, строка конца, индекс
/// компонента)`. Учитываются только документы `Deployment`/`StatefulSet`, чей
/// `metadata.name` (нормализованный, как у `calls::env_calls`) совпадает с
/// заголовком компонента. Для манифестов вне `code_roots`
/// (`kustomize/`, `kubernetes-manifests/`) — детерминированный способ связать
/// условие активации с компонентом, не вводя новых детекторов рёбер `calls`
/// (их ведёт `calls::env_calls`); применяется только к условиям-провайдерам и
/// лишь когда путь не дал владельца.
fn k8s_doc_owners(content: &str, comps: &[ComponentCtx]) -> Vec<(usize, usize, usize)> {
    let mut by_title: BTreeMap<String, usize> = BTreeMap::new();
    for (i, c) in comps.iter().enumerate() {
        by_title
            .entry(super::calls::norm_name(&c.title))
            .or_insert(i);
    }
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut start = 1usize;
    let mut doc = String::new();
    for (idx, line) in lines.iter().enumerate() {
        let lnum = idx + 1;
        if line.trim() == "---" {
            push_doc_owner(&doc, start, lnum.saturating_sub(1), &by_title, &mut out);
            doc.clear();
            start = lnum + 1;
        } else {
            doc.push_str(line);
            doc.push('\n');
        }
    }
    push_doc_owner(&doc, start, lines.len().max(1), &by_title, &mut out);
    // Kustomize-компонент (`kind: Component`) несёт патчи-Deployment'ы внутри
    // блочных скаляров `patch: |-`; они тоже именуют цель `metadata.name`.
    out.extend(embedded_patch_owners(&lines, &by_title));
    out
}

/// Владельцы из блочных скаляров `patch: |…` (kustomize `Component`):
/// каждый блок — отдельный k8s-документ; если это `Deployment`/`StatefulSet`
/// с заголовком компонента — его строки получают владельца.
fn embedded_patch_owners(
    lines: &[&str],
    by_title: &BTreeMap<String, usize>,
) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(key_indent) = patch_block_indent(lines[i]) else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        let mut block = String::new();
        let mut last = i; // 1-based номер последней значимой строки блока
        while j < lines.len() {
            let raw = lines[j];
            let ind = raw.len() - raw.trim_start().len();
            if !raw.trim().is_empty() && ind <= key_indent {
                break;
            }
            block.push_str(raw);
            block.push('\n');
            last = j + 1;
            j += 1;
        }
        if let Some(idx) = doc_yaml_owner(&dedent(&block), by_title) {
            // Содержимое блока — строки i+2 ..= last (1-based).
            out.push((i + 2, last.max(i + 2), idx));
        }
        i = j.max(i + 1);
    }
    out
}

/// Отступ ключа `patch:` (с учётом маркера списка `- `), если строка начинает
/// блочный скаляр (`patch: |…` / `patch: >…`). Иначе `None`.
fn patch_block_indent(line: &str) -> Option<usize> {
    let mut col = line.len() - line.trim_start().len();
    let mut rest = line.trim_start();
    if let Some(r) = rest.strip_prefix("- ") {
        col += 2;
        rest = r;
    }
    let value = rest.strip_prefix("patch:")?.trim_start();
    (value.starts_with('|') || value.starts_with('>')).then_some(col)
}

/// Снимает общий минимальный отступ (блок-скаляр kustomize внутри документа).
fn dedent(block: &str) -> String {
    let min = block
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    block
        .lines()
        .map(|l| l.get(min..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Разбирает один YAML-документ: `Deployment`/`StatefulSet` с заголовком
/// компонента добавляет себя в `out`. Невалидный/чужой документ — молча.
fn push_doc_owner(
    doc: &str,
    start: usize,
    end: usize,
    by_title: &BTreeMap<String, usize>,
    out: &mut Vec<(usize, usize, usize)>,
) {
    if doc.trim().is_empty() {
        return;
    }
    if let Some(idx) = doc_yaml_owner(doc, by_title) {
        out.push((start, end.max(start), idx));
    }
}

/// Индекс компонента по заголовку, если YAML-документ — `Deployment`/`StatefulSet`
/// с совпавшим `metadata.name` (нормализация как в `calls::norm_name`).
fn doc_yaml_owner(doc: &str, by_title: &BTreeMap<String, usize>) -> Option<usize> {
    let v = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(doc).ok()?;
    let map = v.as_mapping()?;
    if !matches!(
        map.get("kind").and_then(serde_yaml_ng::Value::as_str),
        Some("Deployment" | "StatefulSet")
    ) {
        return None;
    }
    let name = map
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(serde_yaml_ng::Value::as_str)?;
    by_title.get(&super::calls::norm_name(name)).copied()
}

/// Владелец, чей k8s-документ покрывает строку `line` (1-based).
fn owner_at(line: usize, docs: &[(usize, usize, usize)]) -> Option<usize> {
    docs.iter()
        .find(|(s, e, _)| line >= *s && line <= *e)
        .map(|(_, _, i)| *i)
}

/// Обращение из конфига: вид узла, id, 1-based строка, условие активации,
/// владелец из k8s-документа (последний — лишь у условий-провайдеров; REC,
/// ADR-075).
type ConfigHit = (NodeKind, String, usize, Option<Condition>, Option<usize>);

/// Рёбра обращений: хранилища, внешние системы-провайдеры и топики из
/// конфигов компонентов. Источники: (1) строки подключения со схемой и
/// `bootstrap.servers`; (2) каталог провайдеров по ключам конфигурации
/// (вложенный YAML → dotted) и по env-маркерам; (3) строки `host:port` без
/// схемы в манифестах с образом хранилища. Денилист B8 отсекает lock-файлы и
/// метаданные; узлы создаются и без владельца (тогда ребра нет).
fn collect_connect_edges(
    snap: &Snapshot,
    comps: &[ComponentCtx],
    acc: &mut EdgeAcc,
    nodes: &mut BTreeMap<String, ArchNode>,
) -> Result<()> {
    let contexts = contexts_of(comps);
    let re_url = Regex::new(INTEGRATION_URL_PATTERN)
        .map_err(|e| HarnessError::Control(format!("внутренний regex интеграций: {e}")))?;
    let re_hostport = Regex::new(MANIFEST_HOST_PORT_PATTERN)
        .map_err(|e| HarnessError::Control(format!("внутренний regex host:port: {e}")))?;
    let redactor = crate::secrets::Redactor::with_builtin_rules();
    let providers = super::providers::Providers::load();
    let deny = crate::survey::SurveyDenylist::builtin();
    for (path, content) in &snap.contents {
        if !looks_like_config(path) || deny.path_ignored(path) {
            continue;
        }
        let owner = owner_of_path(&contexts, path);
        // Окружение overlay-файла (per-environment values) — условие всех
        // рёбер из него (REC, ADR-075, deployment-вид).
        let overlay = overlay_env(path);
        // Владельцы k8s-документов — только когда путь не дал владельца
        // (манифест вне `code_roots`); для условий-провайдеров (REC, ADR-075).
        let doc_owners = if owner.is_none() {
            k8s_doc_owners(content, comps)
        } else {
            Vec::new()
        };
        // (kind, id, 1-based строка, условие активации, владелец из k8s-документа)
        // — все обращения из файла. Владелец-doc заполняется лишь у условий.
        let mut hits: Vec<ConfigHit> = Vec::new();
        // (2a) Провайдер по конфигурационному ключу (вложенный YAML → dotted).
        for (dotted, line) in super::providers::flatten_keys(content) {
            if let Some((def, key)) = providers.match_key(&dotted) {
                let source = Some(Condition {
                    kind: ConditionKind::ConfigKey,
                    text: key.to_string(),
                });
                hits.push((
                    NodeKind::ExternalSystem,
                    format!("sys:{}", def.id),
                    line,
                    hit_condition(overlay, source),
                    owner_at(line, &doc_owners),
                ));
            }
        }
        // (3) Разрешение на схемное-менее разбирательство — образ хранилища.
        let schemeless_scheme = storage_image_scheme(content);
        for (idx, line) in content.lines().enumerate() {
            let lnum = idx + 1;
            if is_comment_line(line) {
                continue;
            }
            // (1a) Строки подключения со схемой (общая механика карты и
            // графа, ADR-069 п.4).
            for hit in crate::survey::integration_hits(&re_url, line) {
                if deny.host_ignored(&hit.host_port) {
                    continue;
                }
                if let Some((kind, id)) = classify_url(&hit, &redactor) {
                    hits.push((kind, id, lnum, hit_condition(overlay, None), None));
                }
            }
            // (2b) Провайдер по сырой подстроке (env-переменная, SDK-маркер).
            if let Some((def, needle)) = providers.match_needle(line) {
                let source = Some(Condition {
                    kind: ConditionKind::EnvFlag,
                    text: needle.to_string(),
                });
                hits.push((
                    NodeKind::ExternalSystem,
                    format!("sys:{}", def.id),
                    lnum,
                    hit_condition(overlay, source),
                    owner_at(lnum, &doc_owners),
                ));
            }
            // (1b) `bootstrap.servers: "k1:9092,k2:9092"` — Kafka без схемы
            // (та же альтернатива, что у детектора `new_datastore`): значение
            // целиком (кавычки срезаются), каждый брокер — свой узел.
            if let Some((_, rest)) = line.split_once("bootstrap.servers") {
                let rest = rest.trim_start_matches([' ', ':', '=']);
                for token in rest.split(',') {
                    let token = token.trim().trim_matches(['"', '\'']);
                    if token.is_empty() {
                        continue;
                    }
                    if let Some(host) = extract_host_port(token) {
                        if deny.host_ignored(&host) {
                            continue;
                        }
                        hits.push((
                            NodeKind::Datastore,
                            redactor.redact(&format!("store:kafka://{host}")),
                            lnum,
                            hit_condition(overlay, None),
                            None,
                        ));
                    }
                }
            }
            // (3) `host:port` без схемы в манифесте с образом хранилища;
            // строку самого образа (`image:`) не разбираем — иначе тег
            // (`postgres:15`) дал бы ложное хранилище.
            if let Some(scheme) = schemeless_scheme {
                if !line.contains("image:") {
                    for m in re_hostport.find_iter(line) {
                        if line[..m.start()].ends_with("//") {
                            continue;
                        }
                        if let Some(host) = extract_host_port(m.as_str()) {
                            if deny.host_ignored(&host) {
                                continue;
                            }
                            hits.push((
                                NodeKind::Datastore,
                                redactor.redact(&format!("store:{scheme}://{host}")),
                                lnum,
                                hit_condition(overlay, None),
                                None,
                            ));
                        }
                    }
                }
            }
        }
        for (kind, id, line, condition, doc_owner) in hits {
            nodes.entry(id.clone()).or_insert_with(|| ArchNode {
                title: node_title(kind, &id),
                id: id.clone(),
                kind,
                inferred: false,
                model_id: None,
            });
            // Владелец пути приоритетнее владельца k8s-документа.
            if let Some(source) = owner.or(doc_owner) {
                acc_edge_cond(
                    acc,
                    &comps[source].id,
                    &id,
                    EdgeKind::Connect,
                    condition,
                    format!("{path}:{line}"),
                );
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
/// Возвращает граф и компоненты с корнями кода (R5: `recover` берёт их для
/// черновика `model/`).
fn build_graph(
    snap: &Snapshot,
    model: Option<&Model>,
    globs: &DiffGlobs,
) -> Result<(ArchGraph, Vec<ComponentCtx>)> {
    let topo = super::modules::deploy_topology(snap);
    let comps = collect_components(snap, model, &topo);
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
    // R2: межсервисные рёбра `calls` по runtime-конфигурации и коду.
    let mut gaps: BTreeSet<String> = BTreeSet::new();
    super::calls::collect_call_edges(snap, &comps, &topo, &mut acc, &mut nodes, &mut gaps)?;

    let mut edges: Vec<ArchEdge> = acc
        .into_iter()
        .map(|((from, to, kind, via, confidence, condition), evidence)| {
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
                via,
                confidence,
                condition,
            }
        })
        .collect();
    edges.sort_by(|a, b| {
        (&a.from, &a.to, a.kind, a.via, a.confidence, &a.condition).cmp(&(
            &b.from,
            &b.to,
            b.kind,
            b.via,
            b.confidence,
            &b.condition,
        ))
    });
    Ok((
        ArchGraph {
            rev: snap.rev.clone(),
            nodes: nodes.into_values().collect(),
            edges,
            gaps: gaps.into_iter().collect(),
        },
        comps,
    ))
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

    /// R1: агрегирующий корень Maven — не компонент; модули получают имена
    /// юнитов развёртывания из `docker-compose`; каталог тестового проекта
    /// (`src/test`) — не компонент.
    #[test]
    fn multi_module_root_not_component() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "pom.xml",
            "<project><modules>\n<module>spring-petclinic-customers-service</module>\n<module>spring-petclinic-vets-service</module>\n</modules></project>\n",
        );
        write_file(
            &repo,
            "spring-petclinic-customers-service/pom.xml",
            "<project/>\n",
        );
        write_file(
            &repo,
            "spring-petclinic-vets-service/pom.xml",
            "<project/>\n",
        );
        write_file(
            &repo,
            "spring-petclinic-customers-service/src/test/pom.xml",
            "<project/>\n",
        );
        write_file(
            &repo,
            "docker-compose.yml",
            "services:\n  customers-service:\n    image: springcommunity/spring-petclinic-customers-service\n  vets-service:\n    image: springcommunity/spring-petclinic-vets-service\n",
        );
        git_repo(&repo);

        let graph = as_built(&repo, "HEAD").expect("граф");
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "dir:spring-petclinic-customers-service",
                "dir:spring-petclinic-vets-service"
            ],
            "{ids:?}"
        );
        let titles: BTreeSet<&str> = graph.nodes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(
            titles,
            BTreeSet::from(["customers-service", "vets-service"]),
            "{titles:?}"
        );
    }

    /// R1 (регресс): `.sln` объявляет проекты путями ФАЙЛОВ; тестовый проект
    /// (`tests\*.tests.csproj`) не должен просачиваться компонентом, а каталог
    /// модуля берёт имя юнита развёртывания (skaffold-контекст → `cartservice`).
    #[test]
    fn sln_test_project_not_component() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "src/cartservice/cartservice.sln",
            "Project(\"{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}\") = \"cartservice\", \"src\\cartservice.csproj\", \"{2348C29F}\"\nEndProject\nProject(\"{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}\") = \"cartservice.tests\", \"tests\\cartservice.tests.csproj\", \"{59825342}\"\nEndProject\n",
        );
        write_file(
            &repo,
            "src/cartservice/src/cartservice.csproj",
            "<Project/>\n",
        );
        write_file(
            &repo,
            "src/cartservice/tests/cartservice.tests.csproj",
            "<Project/>\n",
        );
        write_file(
            &repo,
            "skaffold.yaml",
            "build:\n  artifacts:\n  - image: cartservice\n    context: src/cartservice/src\n",
        );
        write_file(
            &repo,
            "kubernetes-manifests/cartservice.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: cartservice\nspec:\n  template:\n    spec:\n      containers:\n      - name: server\n        image: cartservice\n",
        );
        git_repo(&repo);

        let graph = as_built(&repo, "HEAD").expect("граф");
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["dir:src/cartservice/src"], "{ids:?}");
        let titles: BTreeSet<&str> = graph.nodes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, BTreeSet::from(["cartservice"]), "{titles:?}");
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
            "upstream: \"https://api.bank.ru:8443/v1\"\ndsn: \"postgres://user:secret@ledger-db:5432/ledger\"\nlocal: \"http://127.0.0.1:9000/x\"\nqueue: \"amqp://mq.internal:5672\"\nbootstrap.servers: \"kafka-1:9092,kafka-2:9092\"\nspring:\n  ai:\n    openai:\n      api-key: ${OPENAI_API_KEY}\n",
        );
        git_repo(&repo);
        let graph = as_built(&repo, "HEAD").expect("граф");
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        // Произвольный `host:port` больше не даёт внешней системы (R3, шум).
        assert!(!ids.contains(&"sys:api.bank.ru:8443"), "{ids:?}");
        assert!(!ids.contains(&"sys:mq.internal:5672"), "{ids:?}");
        assert!(ids.contains(&"store:postgres://ledger-db:5432"), "{ids:?}");
        assert!(ids.contains(&"store:kafka://kafka-1:9092"), "{ids:?}");
        assert!(ids.contains(&"store:kafka://kafka-2:9092"), "{ids:?}");
        assert!(!ids.iter().any(|i| i.contains("127.0.0.1")), "{ids:?}");
        assert!(!ids.iter().any(|i| i.contains("secret")), "{ids:?}");
        // Провайдер из каталога (ключ `spring.ai.openai.*` + needle).
        assert!(ids.contains(&"sys:openai"), "{ids:?}");
        let openai = graph
            .nodes
            .iter()
            .find(|n| n.id == "sys:openai")
            .expect("узел провайдера");
        assert_eq!(openai.title, "openai");
        assert_eq!(openai.kind, NodeKind::ExternalSystem);
        let pg = graph
            .edges
            .iter()
            .find(|e| e.to == "store:postgres://ledger-db:5432")
            .expect("ребро к хранилищу");
        assert_eq!(pg.from, "CMP-001");
        assert_eq!(pg.kind, EdgeKind::Connect);
        assert_eq!(pg.evidence, vec!["skeleton/intake/config.yaml:2"]);
    }

    /// Условия активации рёбер `connect` (REC, ADR-075): конфигурационный ключ
    /// провайдера (`config-key`, `@ConditionalOnProperty` через осевой ключ),
    /// env-маркер (`env-flag`) и overlay-файл окружения (`env-overlay`,
    /// deployment-вид — перекрывает построчный источник). Безусловное ребро
    /// (строка подключения) условия не несёт.
    #[test]
    fn connect_edges_carry_activation_conditions() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_python(&repo);
        // Spring-конфиг компонента: ключ Azure OpenAI ⇒ config-key; env-маркер
        // AlloyDB ⇒ env-flag; DSN ⇒ безусловное ребро.
        write_file(
            &repo,
            "skeleton/intake/application.yaml",
            "dsn: \"postgres://ledger-db:5432/ledger\"\nspring:\n  ai:\n    azure:\n      openai:\n        api-key: ${AZURE_SECRET_VALUE}\ncloud:\n  instance: ${ALLOYDB_CLUSTER_NAME}\n",
        );
        // Helm per-environment values: то же направление, что у config-key, но
        // в overlay-файле — условие становится env-overlay (deployment×env).
        write_file(
            &repo,
            "skeleton/intake/deploy/overlays/prod/values-prod.yaml",
            "spring:\n  ai:\n    openai:\n      api-key: ${OPENAI_SECRET_VALUE}\n",
        );
        git_repo(&repo);
        let graph = as_built(&repo, "HEAD").expect("граф");

        let to =
            |id: &str| -> Vec<&ArchEdge> { graph.edges.iter().filter(|e| e.to == id).collect() };

        // config-key: ключ каталога — основание условия.
        let azure = to("sys:azure-openai");
        assert_eq!(azure.len(), 1, "{azure:#?}");
        let cond = azure[0].condition.as_ref().expect("условие config-key");
        assert_eq!(cond.kind, ConditionKind::ConfigKey);
        assert_eq!(cond.text, "spring.ai.azure.openai");

        // env-flag: имя переменной-маркера — основание условия.
        let alloydb = to("sys:alloydb");
        assert_eq!(alloydb.len(), 1, "{alloydb:#?}");
        let cond = alloydb[0].condition.as_ref().expect("условие env-flag");
        assert_eq!(cond.kind, ConditionKind::EnvFlag);
        assert_eq!(cond.text, "ALLOYDB_CLUSTER_NAME");

        // env-overlay: overlay-файл перекрывает построчное условие; ребро одного
        // направления с sys:openai — в окружении prod.
        let openai = to("sys:openai");
        assert_eq!(openai.len(), 1, "{openai:#?}");
        let cond = openai[0].condition.as_ref().expect("условие env-overlay");
        assert_eq!(cond.kind, ConditionKind::EnvOverlay);
        assert_eq!(cond.text, "prod");

        // Безусловное ребро (строка подключения) условия не несёт.
        let pg = to("store:postgres://ledger-db:5432");
        assert_eq!(pg.len(), 1, "{pg:#?}");
        assert!(pg[0].condition.is_none(), "{:#?}", pg[0].condition);
    }

    /// Манифест вне `code_roots` (`kustomize/`, `kubernetes-manifests/`):
    /// условие-провайдер связывается с компонентом по заголовку, совпавшему с
    /// `metadata.name` k8s-документа (`Deployment`/`StatefulSet`, REC, ADR-075).
    /// Путь владельца не даёт (корень компонента — `src/…`), поэтому иначе
    /// ребро-условие потерялось бы (orphan-узел без ребра).
    #[test]
    fn k8s_manifest_attributes_provider_condition_to_component() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        // Компонент с ASCII-заголовком, совпадающим с `metadata.name`.
        write_file(
            &repo,
            "model/CMP-001-assistant.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: shoppingassistantservice\nstatus: adopted\ncode_roots: [src/shoppingassistantservice]\n---\n",
        );
        write_file(
            &repo,
            "src/shoppingassistantservice/lib.rs",
            "pub fn run() {}\n",
        );
        // k8s-манифест вне корня компонента: env-маркер провайдера AlloyDB.
        write_file(
            &repo,
            "kustomize/components/shopping-assistant/shoppingassistantservice.yaml",
            "kind: Deployment\nmetadata:\n  name: shoppingassistantservice\nspec:\n  template:\n    spec:\n      containers:\n        - name: server\n          env:\n            - name: ALLOYDB_CLUSTER_NAME\n              value: \"\"\n",
        );
        git_repo(&repo);
        let graph = as_built(&repo, "HEAD").expect("граф");

        let edge = graph
            .edges
            .iter()
            .find(|e| e.to == "sys:alloydb" && e.kind == EdgeKind::Connect)
            .expect("ребро к alloydb");
        assert_eq!(edge.from, "CMP-001", "{edge:#?}");
        let cond = edge.condition.as_ref().expect("условие env-flag");
        assert_eq!(cond.kind, ConditionKind::EnvFlag);
        assert_eq!(cond.text, "ALLOYDB_CLUSTER_NAME");
    }

    /// Kustomize-компонент (`kind: Component`) с патчем Deployment внутри
    /// блочного скаляра `patch: |-`: условие атрибутируется компоненту по
    /// `metadata.name` вложенного патча (boutique `productcatalog → alloydb`).
    #[test]
    fn kustomize_patch_block_attributes_provider_condition() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "model/CMP-001-catalog.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: productcatalogservice\nstatus: adopted\ncode_roots: [src/productcatalogservice]\n---\n",
        );
        write_file(
            &repo,
            "src/productcatalogservice/lib.rs",
            "pub fn serve() {}\n",
        );
        write_file(
            &repo,
            "kustomize/components/alloydb/kustomization.yaml",
            "apiVersion: kustomize.config.k8s.io/v1alpha1\nkind: Component\npatches:\n- patch: |-\n    apiVersion: apps/v1\n    kind: Deployment\n    metadata:\n      name: productcatalogservice\n    spec:\n      template:\n        spec:\n          containers:\n            - name: server\n              env:\n              - name: ALLOYDB_CLUSTER_NAME\n                value: ALLOYDB_CLUSTER_NAME_VAL\n",
        );
        git_repo(&repo);
        let graph = as_built(&repo, "HEAD").expect("граф");

        let edge = graph
            .edges
            .iter()
            .find(|e| e.to == "sys:alloydb" && e.kind == EdgeKind::Connect)
            .expect("ребро к alloydb");
        assert_eq!(edge.from, "CMP-001", "{edge:#?}");
        let cond = edge.condition.as_ref().expect("условие env-flag");
        assert_eq!(cond.kind, ConditionKind::EnvFlag);
        assert_eq!(cond.text, "ALLOYDB_CLUSTER_NAME");
    }

    /// Хранилище без схемы: `host:port` принимается только в манифесте с
    /// образом хранилища (`REDIS_ADDR=redis-cart:6379` + `image: redis`).
    #[test]
    fn schemeless_storage_requires_image_in_manifest() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_python(&repo);
        write_file(
            &repo,
            "skeleton/intake/deploy.yaml",
            "kind: Deployment\nimage: redis:alpine\nenv:\n  - name: REDIS_ADDR\n    value: \"redis-cart:6379\"\n",
        );
        // Тот же `host:port`, но без образа хранилища рядом — не хранилище.
        write_file(
            &repo,
            "skeleton/intake/plain.yaml",
            "endpoint: \"cache:6379\"\n",
        );
        git_repo(&repo);
        let graph = as_built(&repo, "HEAD").expect("граф");
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"store:redis://redis-cart:6379"), "{ids:?}");
        assert!(!ids.iter().any(|i| i.contains("cache:6379")), "{ids:?}");
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
