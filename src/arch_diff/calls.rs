//! R2: межсервисные связи `calls` по runtime-конфигурации и коду.
//!
//! Четыре независимых извлекателя поверх снимка ревизии — каждый отдаёт рёбра
//! [`EdgeKind::Calls`] с `via` ([`Via`]), основаниями `файл:строка` и
//! достоверностью ([`Confidence`]):
//!
//! - [`env_calls`] — переменные окружения k8s/compose `*_ADDR|*_URL|*_HOST|
//!   *_ENDPOINT` со значением `<имя>:<порт>` или `http(s)://<имя>`
//!   (`via: env`, `confirmed`);
//! - [`spring_calls`] — маршруты gateway `lb://<имя>`, литералы
//!   `http://<имя>/…`, `discoveryClient.getInstances("<имя>")`,
//!   `@FeignClient(name=…)` (`via: route|stub`);
//! - [`grpc_calls`] — определения `service X` в `.proto` (провайдер) и
//!   клиентские стабы (`pb.NewXClient`, `XGrpc.newBlockingStub`,
//!   `new …XClient`) (`via: stub`);
//! - [`topic_calls`] — топики/очереди Kafka/Rabbit (`@KafkaListener(topics=…)`,
//!   `basic_consume(queue=…)`, `producer.send("<topic>")`) — рёбра через
//!   узел-посредник [`NodeKind::Topic`] (`via: topic`, `confirmed`).
//!
//! Имя, не разрешённое в компонент/юнит репозитория, при `gap_on_miss`
//! превращается в `[gap]`-строку «вызов `<имя>` — нет компонента с таким
//! именем» (дедуплицируется множеством, несётся в [`super::types::ArchGraph`]).
//!
//! Границы: core (без сети и LLM, AD-2), read-only по снимку, детерминизм
//! (правило 10: обход `snap.contents` — по `BTreeMap`, порядок стабилен).
//! Файлы, сгенерированные из контрактов (`*.pb.go`, `*_pb2.py`, `*Grpc.java`,
//! каталоги `genproto/`/`generated/`/`gen/`), исключаются — иначе один
//! сгенерированный файл даёт клиентов всех служб и ложные рёбра.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use regex::Regex;
use serde_yaml_ng::Value as Yaml;

use super::as_built::{ComponentCtx, DATASTORE_SCHEMES, EdgeAcc, contexts_of, owner_of_path};
use super::modules::{DeployTopology, split_docs};
use super::snapshot::Snapshot;
use super::types::{ArchNode, Confidence, EdgeKind, NodeKind, Via};
use crate::error::{HarnessError, Result};
use crate::imports::IMPORT_EXTENSIONS;
use crate::survey::CONFIG_EXTENSIONS;

/// Готовит ошибку разбора константного regex (практически недостижимо).
// Адаптер под `map_err(regex_err)` в 14 точках: значение только форматируется,
// брать по ссылке пришлось бы через замыкание в каждой точке — шум без выгоды.
#[allow(clippy::needless_pass_by_value)]
fn regex_err(e: regex::Error) -> HarnessError {
    HarnessError::Control(format!("r2-calls: регулярное выражение: {e}"))
}

/// Расширение пути в нижнем регистре (`""` при отсутствии).
fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// YAML-файл (`.yaml`/`.yml`).
fn is_yaml(path: &str) -> bool {
    let e = ext_of(path);
    e == "yaml" || e == "yml"
}

/// Нормализованное имя для сопоставления: только ASCII-буквоцифры в нижнем
/// регистре (`ProductCatalogService` → `productcatalogservice`,
/// `vets-service` → `vetsservice`). Общая с `as_built` (REC, ADR-075):
/// k8s-имя манифеста сводится к заголовку компонента тем же правилом.
pub(crate) fn norm_name(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Кандидат во внутреннее имя службы: непустое, ≥2 символов, не `localhost`,
/// не только цифры (порт), только `[A-Za-z0-9-]` (без точки — внешние домены
/// отсеиваются).
fn valid_name(s: &str) -> bool {
    if s.len() < 2 {
        return false;
    }
    if s.eq_ignore_ascii_case("localhost") {
        return false;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Имя цели из значения env-переменной: снимает кавычки, схему (`://`),
/// userinfo (`@`), порт и путь; принимает только [`valid_name`].
fn env_target(value: &str) -> Option<String> {
    let mut v = value.trim().trim_matches(|c| c == '"' || c == '\'');
    if let Some(pos) = v.find("://") {
        v = &v[pos + 3..];
    }
    if let Some(pos) = v.find('@') {
        v = &v[pos + 1..];
    }
    let host = v.split([':', '/']).next().unwrap_or(v);
    if valid_name(host) {
        Some(host.to_string())
    } else {
        None
    }
}

/// Имя env-переменной адреса (`*_ADDR|*_URL|*_HOST|*_ENDPOINT`, регистр не важен).
fn is_addr_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["_ADDR", "_URL", "_HOST", "_ENDPOINT"]
        .iter()
        .any(|s| upper.ends_with(s))
}

/// Номер первой строки, содержащей подстроку (1-based; `1` если не найдена).
fn line_of(content: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 1;
    }
    content
        .lines()
        .position(|l| l.contains(needle))
        .map_or(1, |i| i + 1)
}

/// Файл сгенерирован из контракта (`*.pb.go`, `*_pb2*`, `*Grpc.java`, каталог
/// `genproto/`/`generated/`/`gen/`). Такие файлы несут стабы всех служб и
/// должны быть исключены из извлечения рёбер.
fn is_generated(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    if file.contains(".pb.") || file.contains("_pb2") || file.ends_with("grpc.java") {
        return true;
    }
    lower
        .split('/')
        .any(|seg| seg == "genproto" || seg == "generated" || seg == "gen")
}

/// Разрешение имени из основания в узел графа: компонент репозитория (по
/// заголовку) либо юнит-хранилище (образ из [`DATASTORE_SCHEMES`]).
struct Resolver {
    comp_exact: BTreeMap<String, String>,
    comp_norm: BTreeMap<String, String>,
    store_exact: BTreeMap<String, String>,
    store_norm: BTreeMap<String, String>,
}

impl Resolver {
    /// Точные (title) и нормализованные (нормализация регистра/дефисов) карты
    /// компонентов и хранилищ.
    fn new(comps: &[ComponentCtx], topo: &DeployTopology) -> Self {
        let mut comp_exact = BTreeMap::new();
        let mut comp_norm = BTreeMap::new();
        for c in comps {
            comp_exact
                .entry(c.title.clone())
                .or_insert_with(|| c.id.clone());
            comp_norm
                .entry(norm_name(&c.title))
                .or_insert_with(|| c.id.clone());
        }
        let mut store_exact = BTreeMap::new();
        let mut store_norm = BTreeMap::new();
        for u in &topo.units {
            let Some(img) = u.image_base.as_deref() else {
                continue;
            };
            if DATASTORE_SCHEMES.contains(&img) {
                let id = format!("store:{img}://{}", u.name);
                store_exact
                    .entry(u.name.clone())
                    .or_insert_with(|| id.clone());
                store_norm.entry(norm_name(&u.name)).or_insert(id);
            }
        }
        Self {
            comp_exact,
            comp_norm,
            store_exact,
            store_norm,
        }
    }

    /// Разрешение имени: точное совпадение — `confirmed`, нормализованное —
    /// `inferred`; компоненты приоритетнее хранилищ.
    fn resolve(&self, name: &str) -> Option<(String, Confidence)> {
        if let Some(id) = self.comp_exact.get(name) {
            return Some((id.clone(), Confidence::Confirmed));
        }
        if let Some(id) = self.store_exact.get(name) {
            return Some((id.clone(), Confidence::Confirmed));
        }
        let n = norm_name(name);
        if let Some(id) = self.comp_norm.get(&n) {
            return Some((id.clone(), Confidence::Inferred));
        }
        if let Some(id) = self.store_norm.get(&n) {
            return Some((id.clone(), Confidence::Inferred));
        }
        None
    }
}

/// Состояние извлечения рёбер `calls`: накопление рёбер, узлов-посредников
/// (хранилище/топик) и `[gap]`-строк, плюс разрешение имён.
struct Calls<'a> {
    resolver: Resolver,
    /// Компоненты (для поиска владельца файла и источника env-юнита).
    comps: &'a [ComponentCtx],
    /// Контексты владения путём (`code_roots`).
    contexts: Vec<(usize, Vec<String>)>,
    acc: &'a mut EdgeAcc,
    nodes: &'a mut BTreeMap<String, ArchNode>,
    gaps: &'a mut BTreeSet<String>,
}

impl Calls<'_> {
    /// Владелец пути как (id, заголовок) — копии, чтобы не держать заимствование.
    fn owner(&self, path: &str) -> Option<(String, String)> {
        let idx = owner_of_path(&self.contexts, path)?;
        let c = self.comps.get(idx)?;
        Some((c.id.clone(), c.title.clone()))
    }

    /// Компонент-источник по заголовку (имя `Deployment`/ключа compose).
    fn comp_by_title(&self, title: &str) -> Option<(String, String)> {
        self.resolver
            .comp_exact
            .get(title)
            .map(|id| (id.clone(), title.to_string()))
    }

    /// Убедиться, что узел-посредник (хранилище/топик) есть в графе.
    fn ensure_node(&mut self, id: &str) {
        let kind = if id.starts_with("store:") {
            NodeKind::Datastore
        } else if id.starts_with("topic:") {
            NodeKind::Topic
        } else {
            return;
        };
        self.nodes
            .entry(id.to_string())
            .or_insert_with(|| ArchNode {
                id: id.to_string(),
                kind,
                title: id.to_string(),
                inferred: false,
                model_id: None,
            });
    }

    /// Ребро `calls` из источника к цели-имени; при неразрешённом имени и
    /// `gap_on_miss` — `[gap]`-строка. Само-ребро пропускается.
    fn link(
        &mut self,
        from_id: &str,
        from_title: &str,
        target: &str,
        via: Via,
        evidence: String,
        gap_on_miss: bool,
    ) {
        match self.resolver.resolve(target) {
            Some((to_id, confidence)) => {
                if to_id == from_id {
                    return;
                }
                self.ensure_node(&to_id);
                acc_call(self.acc, from_id, &to_id, via, confidence, evidence);
            }
            None => {
                if gap_on_miss {
                    self.gaps.insert(format!(
                        "{from_title}: вызов `{target}` — нет компонента с таким именем"
                    ));
                }
            }
        }
    }

    /// Ребро `calls` к узлу-топику `topic:<имя>` (узел создаётся всегда).
    fn link_topic(&mut self, from_id: &str, topic: &str, evidence: String) {
        let id = format!("topic:{topic}");
        self.ensure_node(&id);
        acc_call(
            self.acc,
            from_id,
            &id,
            Via::Topic,
            Confidence::Confirmed,
            evidence,
        );
    }
}

/// Заносит ребро `calls` в аккумулятор (само-ребро отсекается).
fn acc_call(
    acc: &mut EdgeAcc,
    from: &str,
    to: &str,
    via: Via,
    confidence: Confidence,
    evidence: String,
) {
    if from == to {
        return;
    }
    acc.entry((
        from.to_string(),
        to.to_string(),
        EdgeKind::Calls,
        Some(via),
        Some(confidence),
        None,
    ))
    .or_default()
    .insert(evidence);
}

/// Извлекает переменные окружения контейнера k8s (`spec.template.spec.
/// containers[].env[]`).
fn k8s_env(doc: &Yaml) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(containers) = doc
        .get("spec")
        .and_then(|s| s.get("template"))
        .and_then(|t| t.get("spec"))
        .and_then(|s| s.get("containers"))
        .and_then(Yaml::as_sequence)
    else {
        return out;
    };
    for c in containers {
        let Some(env) = c.get("env").and_then(Yaml::as_sequence) else {
            continue;
        };
        for e in env {
            if let (Some(n), Some(v)) = (
                e.get("name").and_then(Yaml::as_str),
                e.get("value").and_then(Yaml::as_str),
            ) {
                out.push((n.to_string(), v.to_string()));
            }
        }
    }
    out
}

/// Извлекает переменные окружения сервиса compose (`environment:` отображением
/// `NAME: value` или списком `- NAME=value`).
fn compose_env(svc: &Yaml) -> Vec<(String, String)> {
    let mut out = Vec::new();
    match svc.get("environment") {
        Some(Yaml::Mapping(m)) => {
            for (k, v) in m {
                if let (Some(k), Some(v)) = (k.as_str(), v.as_str()) {
                    out.push((k.to_string(), v.to_string()));
                }
            }
        }
        Some(Yaml::Sequence(seq)) => {
            for item in seq {
                if let Some(s) = item.as_str() {
                    if let Some((k, v)) = s.split_once('=') {
                        out.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// Извлекатель env-адресов: k8s `Deployment`/`StatefulSet` и compose
/// `services:`. Источник — [`Calls::comp_by_title`] по `metadata.name`/ключу
/// сервиса; цель — [`env_target`] значения переменной `*_ADDR|*_URL|*_HOST|
/// *_ENDPOINT`.
fn env_calls(snap: &Snapshot, calls: &mut Calls<'_>) {
    for (path, content) in &snap.contents {
        if !is_yaml(path) {
            continue;
        }
        for doc in split_docs(content) {
            let Ok(v) = serde_yaml_ng::from_str::<Yaml>(doc) else {
                continue;
            };
            let Some(map) = v.as_mapping() else {
                continue;
            };
            match map.get("kind").and_then(Yaml::as_str) {
                Some("Deployment" | "StatefulSet") => {
                    let Some(name) = map
                        .get("metadata")
                        .and_then(|m| m.get("name"))
                        .and_then(Yaml::as_str)
                    else {
                        continue;
                    };
                    let Some((src_id, src_title)) = calls.comp_by_title(name) else {
                        continue;
                    };
                    for (env_name, env_value) in k8s_env(&v) {
                        if !is_addr_env(&env_name) {
                            continue;
                        }
                        let Some(target) = env_target(&env_value) else {
                            continue;
                        };
                        let ev = format!("{path}:{}", line_of(content, &env_value));
                        calls.link(&src_id, &src_title, &target, Via::Env, ev, true);
                    }
                }
                None => {
                    let Some(services) = map.get("services").and_then(Yaml::as_mapping) else {
                        continue;
                    };
                    for (k, svc) in services {
                        let Some(name) = k.as_str() else {
                            continue;
                        };
                        let Some((src_id, src_title)) = calls.comp_by_title(name) else {
                            continue;
                        };
                        for (env_name, env_value) in compose_env(svc) {
                            if !is_addr_env(&env_name) {
                                continue;
                            }
                            let Some(target) = env_target(&env_value) else {
                                continue;
                            };
                            let ev = format!("{path}:{}", line_of(content, &env_value));
                            calls.link(&src_id, &src_title, &target, Via::Env, ev, true);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Извлекатель Spring: маршруты gateway `lb://<имя>` (`route`), литералы
/// `http://<имя>/…`, `getInstances("<имя>")`, `@FeignClient(name=…)` (`stub`).
/// Источник — владелец файла по `code_roots`.
fn spring_calls(snap: &Snapshot, calls: &mut Calls<'_>) -> Result<()> {
    let re_route = Regex::new(r"lb://([A-Za-z][A-Za-z0-9-]*)").map_err(regex_err)?;
    let re_http = Regex::new(r"https?://([A-Za-z][A-Za-z0-9-]*)/").map_err(regex_err)?;
    let re_inst = Regex::new(r#"getInstances\s*\(\s*"([^"]+)""#).map_err(regex_err)?;
    let re_feign =
        Regex::new(r#"@FeignClient\s*\(\s*(?:name|value)\s*=\s*"([^"]+)""#).map_err(regex_err)?;
    for (path, content) in &snap.contents {
        let ext = ext_of(path);
        if !(CONFIG_EXTENSIONS.contains(&ext.as_str()) || IMPORT_EXTENSIONS.contains(&ext.as_str()))
        {
            continue;
        }
        let owner = calls.owner(path);
        for (idx, line) in content.lines().enumerate() {
            let ln = idx + 1;
            for cap in re_route.captures_iter(line) {
                emit(calls, &owner, &cap[1], Via::Route, true, path, ln);
            }
            for cap in re_http.captures_iter(line) {
                emit(calls, &owner, &cap[1], Via::Stub, false, path, ln);
            }
            for cap in re_inst.captures_iter(line) {
                emit(calls, &owner, &cap[1], Via::Stub, true, path, ln);
            }
            for cap in re_feign.captures_iter(line) {
                emit(calls, &owner, &cap[1], Via::Stub, true, path, ln);
            }
        }
    }
    Ok(())
}

/// Заносит ребро spring-формы из строки `path:ln`.
fn emit(
    calls: &mut Calls<'_>,
    owner: &Option<(String, String)>,
    name: &str,
    via: Via,
    gap_on_miss: bool,
    path: &str,
    ln: usize,
) {
    let Some((from_id, from_title)) = owner else {
        return;
    };
    calls.link(
        from_id,
        from_title,
        name,
        via,
        format!("{path}:{ln}"),
        gap_on_miss,
    );
}

/// Извлекатель gRPC: провайдеры — имена `service X` в `.proto`; потребители —
/// клиентские стабы `pb.NewXClient`, `XGrpc.newBlockingStub`, `new …XClient`
/// в исходниках (сгенерированные файлы исключены). Ребро — только если имя
/// стаба совпало с провайдером (`stub`).
fn grpc_calls(snap: &Snapshot, calls: &mut Calls<'_>) -> Result<()> {
    let re_service =
        Regex::new(r"(?m)^\s*service\s+([A-Za-z_][A-Za-z0-9_]*)").map_err(regex_err)?;
    let mut providers: BTreeSet<String> = BTreeSet::new();
    for (path, content) in &snap.contents {
        if ext_of(path) != "proto" {
            continue;
        }
        for cap in re_service.captures_iter(content) {
            providers.insert(norm_name(&cap[1]));
        }
    }
    if providers.is_empty() {
        return Ok(());
    }

    let re_go = Regex::new(r"\bNew([A-Z][A-Za-z0-9]*)Client\b").map_err(regex_err)?;
    let re_java =
        Regex::new(r"\b([A-Z][A-Za-z0-9]*)Grpc\s*\.\s*new[A-Za-z]*Stub\b").map_err(regex_err)?;
    let re_cs =
        Regex::new(r"new\s+[A-Za-z_][\w.]*\.([A-Z][A-Za-z0-9]*)Client\b").map_err(regex_err)?;

    for (path, content) in &snap.contents {
        let ext = ext_of(path);
        if !IMPORT_EXTENSIONS.contains(&ext.as_str()) || is_generated(path) {
            continue;
        }
        let owner = calls.owner(path);
        for (idx, line) in content.lines().enumerate() {
            let ln = idx + 1;
            for re in [&re_go, &re_java, &re_cs] {
                for cap in re.captures_iter(line) {
                    let name = &cap[1];
                    if providers.contains(&norm_name(name)) {
                        emit(calls, &owner, name, Via::Stub, true, path, ln);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Извлекатель брокеров: топики/очереди Kafka/Rabbit. Имена `topics={…}`,
/// `queues={…}`, `topics="…"`, `…send("<topic>")`, `basic_consume(queue=…)`
/// превращаются в узел `topic:<имя>` и ребро `via: topic` от владельца файла.
fn topic_calls(snap: &Snapshot, calls: &mut Calls<'_>) -> Result<()> {
    let re_topics = Regex::new(r"topics\s*=\s*\{([^}]*)\}").map_err(regex_err)?;
    let re_topics_single = Regex::new(r#"topics\s*=\s*"([^"]+)""#).map_err(regex_err)?;
    let re_queues = Regex::new(r"queues\s*=\s*\{([^}]*)\}").map_err(regex_err)?;
    let re_quoted = Regex::new(r#""([^"]+)""#).map_err(regex_err)?;
    let re_send = Regex::new(r#"\.send\s*\(\s*"([^"]+)""#).map_err(regex_err)?;
    let re_consume =
        Regex::new(r#"basic_consume\s*\([^)]*queue\s*=\s*['"]([^'"]+)['"]"#).map_err(regex_err)?;
    for (path, content) in &snap.contents {
        let ext = ext_of(path);
        if !IMPORT_EXTENSIONS.contains(&ext.as_str()) || is_generated(path) {
            continue;
        }
        let owner = calls.owner(path);
        let Some((from_id, _)) = owner else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            let ln = idx + 1;
            let ev = || format!("{path}:{ln}");
            for re in [&re_topics, &re_queues] {
                for cap in re.captures_iter(line) {
                    for q in re_quoted.captures_iter(&cap[1]) {
                        calls.link_topic(&from_id, &q[1], ev());
                    }
                }
            }
            for cap in re_topics_single.captures_iter(line) {
                calls.link_topic(&from_id, &cap[1], ev());
            }
            for cap in re_send.captures_iter(line) {
                calls.link_topic(&from_id, &cap[1], ev());
            }
            for cap in re_consume.captures_iter(line) {
                calls.link_topic(&from_id, &cap[1], ev());
            }
        }
    }
    Ok(())
}

/// Собирает все межсервисные рёбра `calls` (R2): env-адреса, Spring-маршруты/
/// стабы, gRPC-стабы, топики брокеров. Неразрешённые имена при необходимости
/// попадают в `gaps`.
///
/// # Errors
/// Константный regex не компилируется (`control`; практически недостижимо).
pub(crate) fn collect_call_edges(
    snap: &Snapshot,
    comps: &[ComponentCtx],
    topo: &DeployTopology,
    acc: &mut EdgeAcc,
    nodes: &mut BTreeMap<String, ArchNode>,
    gaps: &mut BTreeSet<String>,
) -> Result<()> {
    let resolver = Resolver::new(comps, topo);
    let mut calls = Calls {
        resolver,
        comps,
        contexts: contexts_of(comps),
        acc,
        nodes,
        gaps,
    };
    env_calls(snap, &mut calls);
    spring_calls(snap, &mut calls)?;
    grpc_calls(snap, &mut calls)?;
    topic_calls(snap, &mut calls)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch_diff::modules::DeployUnit;

    /// Снимок из пар (путь, содержимое) без git.
    fn snap(files: &[(&str, &str)]) -> Snapshot {
        let mut contents = BTreeMap::new();
        let mut paths = BTreeSet::new();
        for (p, c) in files {
            paths.insert((*p).to_string());
            contents.insert((*p).to_string(), (*c).to_string());
        }
        Snapshot {
            rev: "test".to_string(),
            files: paths,
            contents,
        }
    }

    /// Компонент с одним корнем.
    fn comp(id: &str, title: &str, root: &str) -> ComponentCtx {
        ComponentCtx {
            id: id.to_string(),
            title: title.to_string(),
            inferred: true,
            model_id: None,
            roots: vec![root.to_string()],
        }
    }

    /// Юнит-хранилище (образ из `DATASTORE_SCHEMES`).
    fn store(name: &str, image: &str) -> DeployUnit {
        DeployUnit {
            name: name.to_string(),
            image_base: Some(image.to_string()),
        }
    }

    /// Топология без контекстов и юнитов.
    fn topo(units: Vec<DeployUnit>) -> DeployTopology {
        DeployTopology {
            units,
            contexts: BTreeMap::new(),
        }
    }

    /// Прогон всех извлекателей, возврат рёбер `calls` как (from,to,via,conf).
    fn run(
        snap: &Snapshot,
        comps: &[ComponentCtx],
        tp: &DeployTopology,
    ) -> Vec<(String, String, Via, Confidence)> {
        let mut acc: EdgeAcc = BTreeMap::new();
        let mut nodes: BTreeMap<String, ArchNode> = BTreeMap::new();
        let mut gaps: BTreeSet<String> = BTreeSet::new();
        collect_call_edges(snap, comps, tp, &mut acc, &mut nodes, &mut gaps).unwrap();
        acc.into_iter()
            .map(|((from, to, _k, via, conf, _c), _)| (from, to, via.unwrap(), conf.unwrap()))
            .collect()
    }

    #[test]
    fn env_addr_k8s_edge_confirmed() {
        let s = snap(&[(
            "k8s/checkout.yaml",
            "kind: Deployment\nmetadata:\n  name: checkoutservice\nspec:\n  template:\n    spec:\n      containers:\n      - name: server\n        env:\n        - name: CART_SERVICE_ADDR\n          value: \"cartservice:7070\"\n",
        )]);
        let comps = vec![
            comp(
                "dir:src/checkoutservice",
                "checkoutservice",
                "src/checkoutservice",
            ),
            comp("dir:src/cartservice", "cartservice", "src/cartservice"),
        ];
        let edges = run(&s, &comps, &topo(vec![]));
        assert_eq!(
            edges,
            vec![(
                "dir:src/checkoutservice".to_string(),
                "dir:src/cartservice".to_string(),
                Via::Env,
                Confidence::Confirmed
            )]
        );
    }

    #[test]
    fn env_addr_store_unit_resolves() {
        let s = snap(&[(
            "k8s/cart.yaml",
            "kind: Deployment\nmetadata:\n  name: cartservice\nspec:\n  template:\n    spec:\n      containers:\n      - name: server\n        env:\n        - name: REDIS_ADDR\n          value: \"redis-cart:6379\"\n",
        )]);
        let comps = vec![comp(
            "dir:src/cartservice",
            "cartservice",
            "src/cartservice",
        )];
        let edges = run(&s, &comps, &topo(vec![store("redis-cart", "redis")]));
        assert_eq!(
            edges,
            vec![(
                "dir:src/cartservice".to_string(),
                "store:redis://redis-cart".to_string(),
                Via::Env,
                Confidence::Confirmed
            )]
        );
    }

    #[test]
    fn env_unresolved_name_is_gap() {
        let s = snap(&[(
            "k8s/x.yaml",
            "kind: Deployment\nmetadata:\n  name: frontend\nspec:\n  template:\n    spec:\n      containers:\n      - env:\n        - name: FOO_ADDR\n          value: \"ghost-service:80\"\n",
        )]);
        let comps = vec![comp("dir:src/frontend", "frontend", "src/frontend")];
        let mut acc: EdgeAcc = BTreeMap::new();
        let mut nodes: BTreeMap<String, ArchNode> = BTreeMap::new();
        let mut gaps: BTreeSet<String> = BTreeSet::new();
        collect_call_edges(&s, &comps, &topo(vec![]), &mut acc, &mut nodes, &mut gaps).unwrap();
        assert!(acc.is_empty());
        assert_eq!(
            gaps.into_iter().collect::<Vec<_>>(),
            vec!["frontend: вызов `ghost-service` — нет компонента с таким именем".to_string()]
        );
    }

    #[test]
    fn spring_route_and_http_stub() {
        let s = snap(&[
            (
                "gw/application.yml",
                "spring:\n  cloud:\n    gateway:\n      routes:\n      - uri: lb://vets-service\n",
            ),
            (
                "gen/VectorStoreController.java",
                "package g;\nclass C {\n  void m() { api.uri(\"http://vets-service/x\"); }\n}\n",
            ),
        ]);
        let comps = vec![
            comp("dir:gw", "api-gateway", "gw"),
            comp("dir:gen", "genai-service", "gen"),
            comp("dir:vets", "vets-service", "vets"),
        ];
        let mut edges = run(&s, &comps, &topo(vec![]));
        edges.sort();
        assert_eq!(
            edges,
            vec![
                (
                    "dir:gen".to_string(),
                    "dir:vets".to_string(),
                    Via::Stub,
                    Confidence::Confirmed
                ),
                (
                    "dir:gw".to_string(),
                    "dir:vets".to_string(),
                    Via::Route,
                    Confidence::Confirmed
                ),
            ]
        );
    }

    #[test]
    fn grpc_proto_and_client_stub() {
        let s = snap(&[
            (
                "protos/demo.proto",
                "syntax = \"proto3\";\nservice ProductCatalogService {\n}\n",
            ),
            (
                "src/frontend/rpc.go",
                "package main\nfunc f() { c := pb.NewProductCatalogServiceClient(conn) }\n",
            ),
            (
                "src/frontend/genproto/demo_grpc.pb.go",
                "package demo\nfunc g() { _ = NewEmailServiceClient(nil) }\n",
            ),
        ]);
        let comps = vec![
            comp("dir:src/frontend", "frontend", "src/frontend"),
            comp(
                "dir:src/productcatalogservice",
                "productcatalogservice",
                "src/productcatalogservice",
            ),
        ];
        let edges = run(&s, &comps, &topo(vec![]));
        assert_eq!(
            edges,
            vec![(
                "dir:src/frontend".to_string(),
                "dir:src/productcatalogservice".to_string(),
                Via::Stub,
                Confidence::Inferred
            )]
        );
    }

    #[test]
    fn topic_listener_and_send_edges() {
        let s = snap(&[(
            "src/svc/Listen.java",
            "class L {\n  @KafkaListener(topics = {\"orders\", \"payments\"})\n  void on() {}\n  void s() { producer.send(\"audit\"); }\n}\n",
        )]);
        let comps = vec![comp("dir:src/svc", "svc", "src/svc")];
        let mut edges = run(&s, &comps, &topo(vec![]));
        edges.sort();
        assert_eq!(
            edges,
            vec![
                (
                    "dir:src/svc".to_string(),
                    "topic:audit".to_string(),
                    Via::Topic,
                    Confidence::Confirmed
                ),
                (
                    "dir:src/svc".to_string(),
                    "topic:orders".to_string(),
                    Via::Topic,
                    Confidence::Confirmed
                ),
                (
                    "dir:src/svc".to_string(),
                    "topic:payments".to_string(),
                    Via::Topic,
                    Confidence::Confirmed
                ),
            ]
        );
    }

    #[test]
    fn generated_files_excluded_from_grpc() {
        assert!(is_generated("src/frontend/genproto/demo_grpc.pb.go"));
        assert!(is_generated("a/b/x_pb2.py"));
        assert!(is_generated("com/x/FooGrpc.java"));
        assert!(!is_generated("src/frontend/rpc.go"));
    }

    #[test]
    fn external_url_not_internal_candidate() {
        assert_eq!(env_target("http://github.com/x"), None);
        assert_eq!(env_target("0.0.0.0:8080"), None);
        assert_eq!(
            env_target("redis-cart:6379"),
            Some("redis-cart".to_string())
        );
        assert_eq!(
            env_target("\"shoppingassistantservice:80\""),
            Some("shoppingassistantservice".to_string())
        );
    }
}
