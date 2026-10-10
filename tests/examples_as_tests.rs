//! Образцы `examples/` и демо-сценарии документации — **тестами** (B6):
//! ожидаемый итог зашит рядом с командой, чтобы образец не мог молча
//! разойтись со своей же документацией.
//!
//! Источник сценариев — `docs/corp-spine.md` (§ «Сценарии для демо»):
//! 1. базовый PASS — наследование видно, override активен;
//! 2. «родитель обновился» — error-находка, гейт FAIL до осознанной перепиновки;
//! 3. «override истёк» — warn + правило снова действует → deny-hit, FAIL.

mod common;

use std::path::{Path, PathBuf};

use common::arch_cmd;

/// Копирует `examples/corp-spine` во временный каталог (демо идёт на копии:
/// сценарии 2–3 правят файлы). Возвращает корень демо `…/corp-demo`.
fn copy_corp_demo(home: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/corp-spine");
    let dst = home.join("corp-demo");
    copy_tree(&src, &dst);
    dst
}

/// Рекурсивная копия дерева (без внешних зависимостей).
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir dst");
    for entry in std::fs::read_dir(src).expect("read src") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
}

/// `arch-be control check product --constraints product/CONSTRAINTS.yaml` из
/// корня демо. Возвращает `(exit_code, stdout, stderr)`.
fn control_check_checkout(demo: &Path) -> (i32, String, String) {
    let home = demo.parent().expect("родитель демо = tempdir");
    let out = arch_cmd(home)
        .current_dir(demo)
        .arg("control")
        .arg("check")
        .arg("product")
        .arg("--constraints")
        .arg("product/CONSTRAINTS.yaml")
        .output()
        .expect("прогон arch-be control check");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Сценарий 1 (docs/corp-spine.md): базовый PASS — наследование видно,
/// override активен. Образец `examples/corp-spine` не противоречит своей
/// документации: override `C-CORP-001` узаконен ADR-041 (`Status: Accepted`).
#[test]
fn corp_spine_scenario_1_base_pass() {
    let tmp = tempfile::tempdir().expect("tmp");
    let demo = copy_corp_demo(tmp.path());
    let (code, stdout, stderr) = control_check_checkout(&demo);
    let all = format!("{stdout}{stderr}");
    assert_eq!(code, 0, "сценарий 1 — базовый PASS. Вывод: {all}");
    assert!(all.contains("Итог: PASS"), "ожидался PASS. Вывод: {all}");
    assert!(
        all.contains("[override:active]"),
        "override обязан быть активным. Вывод: {all}"
    );
    assert!(
        !all.contains("override_adr_missing"),
        "ADR-041 обязан быть найден и принят. Вывод: {all}"
    );
    assert!(
        all.contains("CONSTRAINTS.corp@2026.3"),
        "наследование видно (метка источника). Вывод: {all}"
    );
}

/// Сценарий 1 в отчёте вверх (`control report --level corp --json`):
/// `passed: true`, `fail: 0`, override в статусе `active`.
#[test]
fn corp_spine_scenario_1_report_json_active_override() {
    let tmp = tempfile::tempdir().expect("tmp");
    let demo = copy_corp_demo(tmp.path());
    let out = arch_cmd(tmp.path())
        .current_dir(&demo)
        .arg("control")
        .arg("report")
        .arg("product")
        .arg("--constraints")
        .arg("product/CONSTRAINTS.yaml")
        .arg("--level")
        .arg("corp")
        .arg("--json")
        .output()
        .expect("прогон arch-be control report");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("report --json парсится");
    assert_eq!(v["passed"], serde_json::json!(true), "отчёт: {stdout}");
    assert_eq!(v["fail"], serde_json::json!(0), "отчёт: {stdout}");
    assert_eq!(
        v["inherited"]["CONSTRAINTS.corp@2026.3"],
        serde_json::json!(5),
        "все 5 корп-правил наследуются: {stdout}"
    );
    assert_eq!(
        v["overrides"][0]["status"],
        serde_json::json!("active"),
        "override активен: {stdout}"
    );
}

/// Сценарий 2 (docs/corp-spine.md): родитель обновился (version 2026.3 →
/// 2026.4), пин остался 2026.3 → error-находка `extends`, гейт FAIL до
/// осознанной перепиновки.
#[test]
fn corp_spine_scenario_2_parent_bumped_is_fail() {
    let tmp = tempfile::tempdir().expect("tmp");
    let demo = copy_corp_demo(tmp.path());
    let corp = demo.join("CONSTRAINTS.corp.yaml");
    let text = std::fs::read_to_string(&corp).expect("read corp");
    std::fs::write(&corp, text.replace("2026.3", "2026.4")).expect("bump parent");

    let (code, stdout, stderr) = control_check_checkout(&demo);
    let all = format!("{stdout}{stderr}");
    assert_eq!(code, 1, "расхождение пина — гейт FAIL. Вывод: {all}");
    assert!(all.contains("Итог: FAIL"), "ожидался FAIL. Вывод: {all}");
    assert!(
        all.contains("родитель обновился"),
        "нужна находка о расхождении версии. Вывод: {all}"
    );
}

/// Сценарий 3 (docs/corp-spine.md): `until` override в прошлом → warn
/// «override истёк», правило снова действует → deny-hit по `left-pad`, FAIL.
#[test]
fn corp_spine_scenario_3_expired_override_is_fail() {
    let tmp = tempfile::tempdir().expect("tmp");
    let demo = copy_corp_demo(tmp.path());
    let prod = demo.join("product/CONSTRAINTS.yaml");
    let text = std::fs::read_to_string(&prod).expect("read product");
    std::fs::write(&prod, text.replace("2027-01", "2025-01")).expect("expire override");

    let (code, stdout, stderr) = control_check_checkout(&demo);
    let all = format!("{stdout}{stderr}");
    assert_eq!(
        code, 1,
        "истёкший override возвращает правило. Вывод: {all}"
    );
    assert!(all.contains("Итог: FAIL"), "ожидался FAIL. Вывод: {all}");
    assert!(
        all.contains("override истёк"),
        "нужна warn-находка о протухшем override. Вывод: {all}"
    );
    assert!(
        all.contains("deny_dependency") && all.contains("left-pad"),
        "правило снова действует — deny-hit. Вывод: {all}"
    );
}
