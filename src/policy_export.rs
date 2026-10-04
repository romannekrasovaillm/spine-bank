//! Экспорт инвариантов развёртывания в политики кластера (ADR-059 §D3,
//! волна D). Мост «спайн → политики кластера» по аналогии с `ArchUnit`-мостом
//! (ADR-039): архитектор описывает инварианты деплоя один раз — отдельной
//! секцией `deployment:` в `CONSTRAINTS.yaml` (правило 4 TASK: формат `rules:`
//! не трогается вовсе), а `arch-be policy export kyverno|rego` проецирует их
//! в проверяемые политики кластера.
//!
//! Границы:
//!
//! - секция `deployment:` **толерантна** ко всем существующим читателям
//!   реестра (`gate`, `fitness`, `rules_report`, delta-guard, handoff-копия):
//!   serde по умолчанию игнорирует неизвестные top-level-поля, поэтому гейт
//!   её просто не видит и работает как без неё;
//! - экспорт детерминирован: вывод — чистая функция секции, golden-тесты
//!   фиксируют байты; `deny_images` канонизируется (сортировка + дедуп);
//! - сеть/TUI не используются (граница C-34), YAML — через уже имеющийся
//!   `serde_yaml_ng`; вывод политик — стабильные шаблоны.
//!
//! Схема секции (все поля опциональны, кроме идентифицирующих; неизвестные
//! поля внутри `deployment:` игнорируются):
//!
//! ```yaml
//! deployment:
//!   images:
//!     registry: "registry.example.com/bank"  # разрешённый префикс реестра
//!     signed: true                            # подпись образа обязательна
//!   security:
//!     run_as_non_root: true                   # securityContext.runAsNonRoot
//!   resources:
//!     max_cpu: "500m"                         # k8s-квантор, потолок limits.cpu
//!     max_memory: "512Mi"                     # k8s-квантор, потолок limits.memory
//!   deny_images:                              # tech-радар для образов
//!     - "docker.io/library/nginx"
//! ```
//!
//! Схема и примеры — `docs/policy-export.md`.

use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;

use crate::error::{HarnessError, Result};

/// Корень `CONSTRAINTS.yaml` глазами экспортёра: интересует только секция
/// `deployment:`, остальные top-level-ключи (`rules:`, `extends:`, …)
/// игнорируются serde — толерантность к чужим полям заложена самим форматом.
#[derive(Debug, Default, Deserialize)]
struct DeploymentFile {
    /// Секция инвариантов развёртывания (отсутствует — `None`).
    #[serde(default)]
    deployment: Option<DeploymentPolicy>,
}

/// Инварианты развёртывания (секция `deployment:`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DeploymentPolicy {
    /// Образы: реестр и подпись.
    #[serde(default)]
    pub images: ImagesPolicy,
    /// Безопасность контейнера.
    #[serde(default)]
    pub security: SecurityPolicy,
    /// Лимиты ресурсов.
    #[serde(default)]
    pub resources: ResourcesPolicy,
    /// deny-список образов (tech-радар `deny_dependency` для образов).
    #[serde(default)]
    pub deny_images: Vec<String>,
}

/// Инварианты по образам.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ImagesPolicy {
    /// Разрешённый префикс реестра (`registry.example.com/bank`).
    #[serde(default)]
    pub registry: Option<String>,
    /// Подпись образа обязательна.
    #[serde(default)]
    pub signed: Option<bool>,
}

/// Инварианты безопасности контейнера.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SecurityPolicy {
    /// Контейнеры обязаны запускаться от non-root.
    #[serde(default)]
    pub run_as_non_root: Option<bool>,
}

/// Потолки ресурсов контейнера (k8s-кванторы как строки).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ResourcesPolicy {
    /// Потолок `limits.cpu` (например `500m`, `2`).
    #[serde(default)]
    pub max_cpu: Option<String>,
    /// Потолок `limits.memory` (например `512Mi`, `1Gi`).
    #[serde(default)]
    pub max_memory: Option<String>,
}

impl DeploymentPolicy {
    /// Есть ли хоть один действующий инвариант. `signed: false` и
    /// `run_as_non_root: false` — снятые ограничения, политик не порождают;
    /// отсюда «пустая секция» и «отсутствующая секция» одинаково нечего
    /// экспортировать.
    #[must_use]
    pub fn has_invariants(&self) -> bool {
        self.images.registry.is_some()
            || self.images.signed == Some(true)
            || self.security.run_as_non_root == Some(true)
            || self.resources.max_cpu.is_some()
            || self.resources.max_memory.is_some()
            || !self.deny_images.is_empty()
    }
}

/// Формат экспортируемой политики.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Kyverno `ClusterPolicy` (`kyverno.io/v1`).
    Kyverno,
    /// OPA/Conftest Rego (пакет `archbe.deployment`, Rego v1).
    Rego,
}

/// Разбирает секцию `deployment:` из текста `CONSTRAINTS.yaml`.
///
/// # Errors
/// YAML невалиден или секция `deployment:` не соответствует схеме (битые
/// типы полей). Прочие секции файла не разбираются и на результат не влияют.
pub fn parse(yaml: &str) -> Result<Option<DeploymentPolicy>> {
    let file: DeploymentFile = serde_yaml_ng::from_str(yaml)?;
    Ok(file.deployment)
}

/// Читает секцию `deployment:` из файла `CONSTRAINTS.yaml`.
///
/// # Errors
/// Файл не читается, YAML невалиден или секция не соответствует схеме.
pub fn load(constraints: &Path) -> Result<Option<DeploymentPolicy>> {
    let yaml =
        std::fs::read_to_string(constraints).map_err(|e| HarnessError::io(constraints, e))?;
    parse(&yaml)
}

/// Экспортирует инварианты в заданный формат. Детерминированная чистая
/// функция секции: один и тот же вход даёт байт-в-байт одинаковый выход.
#[must_use]
pub fn export(policy: &DeploymentPolicy, format: ExportFormat) -> String {
    match format {
        ExportFormat::Kyverno => export_kyverno(policy),
        ExportFormat::Rego => export_rego(policy),
    }
}

// ---------------------------------------------------------------------------
// Kyverno
// ---------------------------------------------------------------------------

/// Собирает YAML-манифесты Kyverno: по одному `ClusterPolicy` на группу
/// инвариантов, документы разделены `---`. Порядок групп фиксирован.
fn export_kyverno(policy: &DeploymentPolicy) -> String {
    let mut docs: Vec<String> = Vec::new();
    if let Some(doc) = kyverno_images(policy) {
        docs.push(doc);
    }
    if policy.security.run_as_non_root == Some(true) {
        docs.push(kyverno_non_root());
    }
    if let Some(doc) = kyverno_resources(policy) {
        docs.push(doc);
    }
    if !policy.deny_images.is_empty() {
        docs.push(kyverno_deny_images(policy));
    }
    let mut out = String::new();
    for (index, doc) in docs.iter().enumerate() {
        if index > 0 {
            out.push_str("---\n");
        }
        out.push_str(doc);
    }
    out
}

/// Шапка `ClusterPolicy`: имя, аннотации-источники, политика Enforce.
fn kyverno_header(out: &mut String, name: &str, group: &str) {
    let _ = writeln!(out, "apiVersion: kyverno.io/v1");
    let _ = writeln!(out, "kind: ClusterPolicy");
    let _ = writeln!(out, "metadata:");
    let _ = writeln!(out, "  name: {name}");
    let _ = writeln!(out, "  annotations:");
    let _ = writeln!(out, "    arch-be.spine/source: CONSTRAINTS.yaml");
    let _ = writeln!(out, "    arch-be.spine/group: {group}");
    let _ = writeln!(out, "spec:");
    let _ = writeln!(out, "  validationFailureAction: Enforce");
    let _ = writeln!(out, "  background: true");
    let _ = writeln!(out, "  rules:");
}

/// Общий блок `match` (kinds: Pod) на заданном отступе.
fn kyverno_match(out: &mut String, indent: &str) {
    let _ = writeln!(out, "{indent}match:");
    let _ = writeln!(out, "{indent}  any:");
    let _ = writeln!(out, "{indent}    - resources:");
    let _ = writeln!(out, "{indent}        kinds:");
    let _ = writeln!(out, "{indent}          - Pod");
}

/// Группа «образы»: `validate` по префиксу реестра и `verifyImages` при
/// `signed: true`.
fn kyverno_images(policy: &DeploymentPolicy) -> Option<String> {
    let registry = policy.images.registry.as_deref();
    let signed = policy.images.signed == Some(true);
    if registry.is_none() && !signed {
        return None;
    }
    let mut out = String::new();
    kyverno_header(&mut out, "archbe-deploy-images", "images");
    if let Some(reg) = registry {
        let _ = writeln!(out, "    - name: restrict-image-registry");
        kyverno_match(&mut out, "      ");
        let _ = writeln!(out, "      validate:");
        let _ = writeln!(
            out,
            "        message: \"образы контейнеров должны происходить из реестра {reg}\""
        );
        let _ = writeln!(out, "        foreach:");
        for list in [
            "request.object.spec.containers",
            "request.object.spec.initContainers",
        ] {
            let _ = writeln!(out, "          - list: {list}");
            let _ = writeln!(out, "            pattern:");
            let _ = writeln!(out, "              image: \"{reg}/*\"");
        }
    }
    if signed {
        let refs = registry.map_or_else(|| "*".to_string(), |reg| format!("{reg}/*"));
        let _ = writeln!(out, "    - name: verify-image-signature");
        kyverno_match(&mut out, "      ");
        let _ = writeln!(out, "      verifyImages:");
        let _ = writeln!(out, "        - imageReferences:");
        let _ = writeln!(out, "            - \"{refs}\"");
        let _ = writeln!(out, "          attestors:");
        let _ = writeln!(out, "            - entries:");
        let _ = writeln!(out, "                - keys:");
        let _ = writeln!(
            out,
            "                    publicKeys: \"k8s://arch-be/image-signing-key\""
        );
        let _ = writeln!(
            out,
            "                    # замените на реальный публичный ключ проверки подписи (cosign)"
        );
    }
    Some(out)
}

/// Группа «non-root»: `securityContext.runAsNonRoot: true` у всех контейнеров.
fn kyverno_non_root() -> String {
    let mut out = String::new();
    kyverno_header(&mut out, "archbe-deploy-nonroot", "security");
    let _ = writeln!(out, "    - name: require-run-as-non-root");
    kyverno_match(&mut out, "      ");
    let _ = writeln!(out, "      validate:");
    let _ = writeln!(
        out,
        "        message: \"контейнеры должны запускаться от non-root (securityContext.runAsNonRoot: true)\""
    );
    let _ = writeln!(out, "        foreach:");
    for list in [
        "request.object.spec.containers",
        "request.object.spec.initContainers",
    ] {
        let _ = writeln!(out, "          - list: {list}");
        let _ = writeln!(out, "            pattern:");
        let _ = writeln!(out, "              securityContext:");
        let _ = writeln!(out, "                runAsNonRoot: true");
    }
    out
}

/// Группа «лимиты»: `limits.cpu`/`limits.memory` не превышают бюджет
/// (операторы сравнения Kyverno понимают k8s-кванторы).
fn kyverno_resources(policy: &DeploymentPolicy) -> Option<String> {
    let mut limits: Vec<String> = Vec::new();
    let mut budget: Vec<String> = Vec::new();
    if let Some(cpu) = &policy.resources.max_cpu {
        limits.push(format!("                  cpu: \"<={cpu}\""));
        budget.push(format!("cpu ≤ {cpu}"));
    }
    if let Some(memory) = &policy.resources.max_memory {
        limits.push(format!("                  memory: \"<={memory}\""));
        budget.push(format!("memory ≤ {memory}"));
    }
    if limits.is_empty() {
        return None;
    }
    let budget = budget.join(", ");
    let mut out = String::new();
    kyverno_header(&mut out, "archbe-deploy-resources", "resources");
    let _ = writeln!(out, "    - name: require-resource-limits");
    kyverno_match(&mut out, "      ");
    let _ = writeln!(out, "      validate:");
    let _ = writeln!(
        out,
        "        message: \"limits контейнеров не должны превышать бюджет ({budget})\""
    );
    let _ = writeln!(out, "        foreach:");
    for list in [
        "request.object.spec.containers",
        "request.object.spec.initContainers",
    ] {
        let _ = writeln!(out, "          - list: {list}");
        let _ = writeln!(out, "            pattern:");
        let _ = writeln!(out, "              resources:");
        let _ = writeln!(out, "                limits:");
        for line in &limits {
            let _ = writeln!(out, "{line}");
        }
    }
    Some(out)
}

/// Группа «deny-список образов»: по правилу на запись списка — `image` не
/// начинается с запрещённой ссылки (негативный glob-паттерн, AND по записям).
fn kyverno_deny_images(policy: &DeploymentPolicy) -> String {
    let images = canonical_images(&policy.deny_images);
    let mut out = String::new();
    kyverno_header(&mut out, "archbe-deploy-deny-images", "deny-images");
    for (index, image) in images.iter().enumerate() {
        let _ = writeln!(out, "    - name: deny-image-{index}");
        kyverno_match(&mut out, "      ");
        let _ = writeln!(out, "      validate:");
        let _ = writeln!(
            out,
            "        message: \"образ из deny-списка развёртывания запрещён: {image}\""
        );
        let _ = writeln!(out, "        foreach:");
        for list in [
            "request.object.spec.containers",
            "request.object.spec.initContainers",
        ] {
            let _ = writeln!(out, "          - list: {list}");
            let _ = writeln!(out, "            pattern:");
            let _ = writeln!(out, "              image: \"!{image}*\"");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Rego
// ---------------------------------------------------------------------------

/// Собирает один файл Rego (пакет `archbe.deployment`, Rego v1): `deny` —
/// множество нарушений, `allow` — объект чист. Порядок групп фиксирован.
fn export_rego(policy: &DeploymentPolicy) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Сгенерировано `arch-be policy export rego` из секции `deployment:` CONSTRAINTS.yaml."
    );
    let _ = writeln!(
        out,
        "# Пакет для OPA/Conftest (Rego v1): объект проходит, если множество `deny` пусто."
    );
    let _ = writeln!(out, "package archbe.deployment");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "# Все контейнеры Pod: spec.containers + spec.initContainers."
    );
    let _ = writeln!(
        out,
        "containers := array.concat(spec_list(\"containers\"), spec_list(\"initContainers\"))"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "spec_list(key) := object.get(object.get(input, \"spec\", {{}}), key, [])"
    );
    let _ = writeln!(out);
    if let Some(registry) = &policy.images.registry {
        rego_registry(&mut out, registry);
    }
    if policy.images.signed == Some(true) {
        rego_signed(&mut out);
    }
    if policy.security.run_as_non_root == Some(true) {
        rego_non_root(&mut out);
    }
    rego_resources(&mut out, &policy.resources);
    if !policy.deny_images.is_empty() {
        rego_deny_images(&mut out, &policy.deny_images);
    }
    let _ = writeln!(out, "# allow — вердикт по объекту: нарушений нет.");
    let _ = writeln!(out, "allow if {{");
    let _ = writeln!(out, "    count(deny) == 0");
    let _ = writeln!(out, "}}");
    out
}

/// Правило «образы только из реестра».
fn rego_registry(out: &mut String, registry: &str) {
    let _ = writeln!(out, "# Инвариант: образы только из внутреннего реестра.");
    let _ = writeln!(out, "deny contains msg if {{");
    let _ = writeln!(out, "    some c in containers");
    let _ = writeln!(out, "    not startswith(c.image, \"{registry}/\")");
    let _ = writeln!(
        out,
        "    msg := sprintf(\"образ %q вне разрешённого реестра %q\", [c.image, \"{registry}/\"])"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Правило «образы закреплены по дайджесту» (подпись). Plain Rego не
/// проверяет cosign-подписи — криптопроверку отдаёт Kyverno `verifyImages`;
/// здесь удерживается её следствие — адресация по дайджесту.
fn rego_signed(out: &mut String) {
    let _ = writeln!(
        out,
        "# Инвариант: образы подписаны. Криптопроверка cosign — за Kyverno verifyImages;"
    );
    let _ = writeln!(
        out,
        "# здесь удерживается следствие подписанной поставки — адресация по дайджесту."
    );
    let _ = writeln!(out, "deny contains msg if {{");
    let _ = writeln!(out, "    some c in containers");
    let _ = writeln!(
        out,
        "    not regex.match(\"^[^@]+@sha256:[0-9a-f]{{64}}$\", c.image)"
    );
    let _ = writeln!(
        out,
        "    msg := sprintf(\"образ %q не закреплён по дайджесту (@sha256:)\", [c.image])"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Правило «контейнеры от non-root».
fn rego_non_root(out: &mut String) {
    let _ = writeln!(out, "# Инвариант: контейнеры работают от non-root.");
    let _ = writeln!(out, "deny contains msg if {{");
    let _ = writeln!(out, "    some c in containers");
    let _ = writeln!(out, "    not pod_non_root");
    let _ = writeln!(out, "    not container_non_root(c)");
    let _ = writeln!(
        out,
        "    msg := sprintf(\"контейнер %q: securityContext.runAsNonRoot обязателен\", [object.get(c, \"name\", \"<без имени>\")])"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "pod_non_root if {{");
    let _ = writeln!(out, "    input.spec.securityContext.runAsNonRoot == true");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "container_non_root(c) if {{");
    let _ = writeln!(out, "    c.securityContext.runAsNonRoot == true");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Правило «limits в пределах бюджета».
fn rego_resources(out: &mut String, resources: &ResourcesPolicy) {
    let mut checks: Vec<String> = Vec::new();
    let mut budget: Vec<String> = Vec::new();
    if let Some(cpu) = &resources.max_cpu {
        checks.push(format!(
            "    units.parse(sprintf(\"%v\", [c.resources.limits.cpu])) <= units.parse(\"{cpu}\")"
        ));
        budget.push(format!("cpu <= {cpu}"));
    }
    if let Some(memory) = &resources.max_memory {
        checks.push(format!(
            "    units.parse_bytes(sprintf(\"%v\", [c.resources.limits.memory])) <= units.parse_bytes(\"{memory}\")"
        ));
        budget.push(format!("memory <= {memory}"));
    }
    if checks.is_empty() {
        return;
    }
    let budget = budget.join(", ");
    let _ = writeln!(out, "# Инвариант: limits не заданы или превышают бюджет.");
    let _ = writeln!(out, "deny contains msg if {{");
    let _ = writeln!(out, "    some c in containers");
    let _ = writeln!(out, "    not limits_within_budget(c)");
    let _ = writeln!(
        out,
        "    msg := sprintf(\"контейнер %q: limits не заданы или превышают бюджет ({budget})\", [object.get(c, \"name\", \"<без имени>\")])"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "limits_within_budget(c) if {{");
    for check in &checks {
        let _ = writeln!(out, "{check}");
    }
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Правило «deny-список образов».
fn rego_deny_images(out: &mut String, deny_images: &[String]) {
    let images = canonical_images(deny_images);
    let _ = writeln!(out, "# Инвариант: deny-список образов.");
    let _ = writeln!(out, "deny_images := [");
    for image in &images {
        let _ = writeln!(out, "    \"{image}\",");
    }
    let _ = writeln!(out, "]");
    let _ = writeln!(out);
    let _ = writeln!(out, "deny contains msg if {{");
    let _ = writeln!(out, "    some c in containers");
    let _ = writeln!(out, "    image_denied(c.image)");
    let _ = writeln!(
        out,
        "    msg := sprintf(\"образ %q в deny-списке развёртывания\", [c.image])"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "image_denied(image) if {{");
    let _ = writeln!(out, "    some d in deny_images");
    let _ = writeln!(out, "    image == d");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "image_denied(image) if {{");
    let _ = writeln!(out, "    some d in deny_images");
    let _ = writeln!(out, "    startswith(image, sprintf(\"%s:\", [d]))");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "image_denied(image) if {{");
    let _ = writeln!(out, "    some d in deny_images");
    let _ = writeln!(out, "    startswith(image, sprintf(\"%s@\", [d]))");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// Канонический вид deny-списка: сортировка + дедуп (детерминизм вывода
/// независимо от порядка авторства).
fn canonical_images(images: &[String]) -> Vec<String> {
    let mut list = images.to_vec();
    list.sort();
    list.dedup();
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "\
rules:
  - name: exists
    type: file_exists
    path: \"Cargo.toml\"
    severity: error
deployment:
  images:
    registry: \"registry.example.com/bank\"
    signed: true
  security:
    run_as_non_root: true
  resources:
    max_cpu: \"500m\"
    max_memory: \"512Mi\"
  deny_images:
    - \"docker.io/library/redis\"
    - \"docker.io/library/nginx\"
";

    #[test]
    fn parses_deployment_section_and_ignores_other_roots() {
        let policy = parse(FULL).expect("разбор").expect("секция");
        assert_eq!(
            policy.images.registry.as_deref(),
            Some("registry.example.com/bank")
        );
        assert_eq!(policy.images.signed, Some(true));
        assert_eq!(policy.security.run_as_non_root, Some(true));
        assert_eq!(policy.resources.max_cpu.as_deref(), Some("500m"));
        assert_eq!(policy.resources.max_memory.as_deref(), Some("512Mi"));
        assert_eq!(policy.deny_images.len(), 2);
        assert!(policy.has_invariants());
    }

    #[test]
    fn absent_section_is_none_and_other_sections_parse() {
        let policy = parse("rules: []\nextends: [\"corp.yaml@1\"]\n").expect("разбор");
        assert!(policy.is_none());
    }

    #[test]
    fn empty_section_has_no_invariants() {
        let policy = parse("deployment: {}\n").expect("разбор").expect("секция");
        assert!(!policy.has_invariants());
        // Снятые ограничения не порождают правил.
        let relaxed = parse(
            "deployment:\n  images:\n    signed: false\n  security:\n    run_as_non_root: false\n",
        )
        .expect("разбор")
        .expect("секция");
        assert!(!relaxed.has_invariants());
    }

    #[test]
    fn kyverno_export_is_deterministic_and_has_all_groups() {
        let policy = parse(FULL).expect("разбор").expect("секция");
        let first = export(&policy, ExportFormat::Kyverno);
        let second = export(&policy, ExportFormat::Kyverno);
        assert_eq!(first, second, "повторный экспорт идентичен");
        for marker in [
            "kind: ClusterPolicy",
            "name: archbe-deploy-images",
            "name: archbe-deploy-nonroot",
            "name: archbe-deploy-resources",
            "name: archbe-deploy-deny-images",
            "verifyImages:",
            "registry.example.com/bank/*",
            "cpu: \"<=500m\"",
            "memory: \"<=512Mi\"",
        ] {
            assert!(first.contains(marker), "нет маркера: {marker}");
        }
        // Каждый манифест — валидный YAML-документ с ожидаемым kind.
        for doc in first.split("---\n") {
            let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(doc).expect("валидный YAML");
            assert_eq!(
                value.get("kind").and_then(|k| k.as_str()),
                Some("ClusterPolicy")
            );
        }
    }

    #[test]
    fn rego_export_is_deterministic_and_has_all_groups() {
        let policy = parse(FULL).expect("разбор").expect("секция");
        let first = export(&policy, ExportFormat::Rego);
        let second = export(&policy, ExportFormat::Rego);
        assert_eq!(first, second, "повторный экспорт идентичен");
        for marker in [
            "package archbe.deployment",
            "deny contains msg if",
            "allow if",
            "registry.example.com/bank/",
            "limits_within_budget",
            "pod_non_root",
            "deny_images := [",
        ] {
            assert!(first.contains(marker), "нет маркера: {marker}");
        }
        // deny-список канонизирован (сортировка), а не в порядке авторства.
        let nginx = first.find("docker.io/library/nginx").expect("nginx");
        let redis = first.find("docker.io/library/redis").expect("redis");
        assert!(nginx < redis, "deny-список отсортирован");
    }

    /// Golden: полный набор инвариантов → байт-в-байт замороженный вывод
    /// (любое изменение шаблонов видно здесь, а не в runtime).
    #[test]
    fn golden_fixture_matches_expected_output() {
        let yaml = include_str!("../tests/fixtures/policy-export/CONSTRAINTS.yaml");
        let policy = parse(yaml).expect("разбор").expect("секция");
        assert_eq!(
            export(&policy, ExportFormat::Kyverno),
            include_str!("../tests/fixtures/policy-export/expected.kyverno.yaml")
        );
        assert_eq!(
            export(&policy, ExportFormat::Rego),
            include_str!("../tests/fixtures/policy-export/expected.rego")
        );
    }

    #[test]
    fn minimal_policy_renders_only_present_groups() {
        let policy = parse("deployment:\n  images:\n    registry: \"r.io\"\n")
            .expect("разбор")
            .expect("секция");
        let kyverno = export(&policy, ExportFormat::Kyverno);
        assert!(kyverno.contains("archbe-deploy-images"));
        assert!(!kyverno.contains("archbe-deploy-nonroot"));
        assert!(!kyverno.contains("archbe-deploy-resources"));
        assert!(!kyverno.contains("archbe-deploy-deny-images"));
        assert!(!kyverno.contains("verifyImages:"));
    }
}
