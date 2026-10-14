//! Бенчмарк `benchmarks/icl-transfer` — детерминированный прогон `--ci`.
//!
//! Проверяет механику обучающего контура без сети и без живой LLM:
//! генератор пула (детерминированный по семени), обобщённый гейт,
//! `FakeLlm` + fake-harness и руки FI+L/FI/X/GEN/PAIR/PROMO/FLEET.
//! Пороги заморожены в `benchmarks/icl-transfer/PREREGISTRATION.md` §4.
//!
//! Анти-Goodhart: маркерные строки пула запрещены в `src/` и `assets/`
//! (TASK-Spine-0.5.0-learning-loop.md, правило 9). Тест гонит греп
//! `--check-antigoodhart` и дополнительно читает флаг `antigoodhart.passed`
//! из сводки. Конкретные конвенции пула здесь НЕ упоминаются (только id из
//! файла `prereg/pool.yaml` во время прогона).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Абсолютный путь к python3 (whitelist может не иметь PATH).
fn python() -> String {
    for candidate in ["/usr/bin/python3", "/usr/local/bin/python3"] {
        if Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }
    String::from("python3")
}

fn python3_available() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("python3").is_file()))
}

fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("benchmarks")
        .join("icl-transfer")
}

fn runner() -> PathBuf {
    bench_dir().join("runners").join("iclbench.py")
}

/// Запускает `python3 iclbench.py <args…>`, возвращает (exit-код, stdout).
fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(python())
        .arg(runner())
        .args(args)
        .output()
        .expect("запуск iclbench.py");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn json_field(v: &Value, arm: &str, field: &str) -> f64 {
    v["arms"][arm][field]
        .as_f64()
        .unwrap_or_else(|| panic!("arms.{arm}.{field} — не число: {v}"))
}

/// Детерминизм: два прогона `--ci` дают байт-в-байт одинаковую сводку.
#[test]
fn ci_is_deterministic() {
    if !python3_available() {
        eprintln!("skipped: no python3");
        return;
    }
    let (rc1, out1) = run(&["--ci"]);
    assert_eq!(rc1, 0, "первый --ci: {out1}");
    let (rc2, out2) = run(&["--ci"]);
    assert_eq!(rc2, 0, "второй --ci: {out2}");
    assert_eq!(out1, out2, "--ci недетерминирован");
}

/// Генератор пула детерминирован и не выбирает holdout-конвенции.
#[test]
fn generator_is_deterministic_and_excludes_holdout() {
    if !python3_available() {
        eprintln!("skipped: no python3");
        return;
    }
    let (rc, out) = run(&["--gen", "141"]);
    assert_eq!(rc, 0, "--gen 141: {out}");
    let v: Value = serde_json::from_str(&out).expect("--gen вернул не JSON");
    let convs = v["conventions"].as_array().expect("conventions — массив");
    assert!((2..=3).contains(&convs.len()), "число конвенций: {convs:?}");
    // holdout-конвенции (по id из файла) не выбираются — их состав здесь не знаем.
    let ids = v["conventions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let (_, gen2) = run(&["--gen", "141"]);
    let v2: Value = serde_json::from_str(&gen2).expect("--gen повторил JSON");
    let ids2 = v2["conventions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(ids, ids2, "--gen недетерминирован");
}

/// Все руки --ci отрабатывают и дают механику в пределах замороженных порогов.
#[test]
fn ci_arms_within_thresholds() {
    if !python3_available() {
        eprintln!("skipped: no python3");
        return;
    }
    let (rc, out) = run(&["--ci"]);
    assert_eq!(rc, 0, "--ci: {out}");
    let v: Value = serde_json::from_str(&out).expect("--ci вернул не JSON");
    assert_eq!(v["mode"], "ci");

    // Анти-Goodhart: греп чист и не вакуумен (маркеры в пуле есть).
    assert_eq!(v["antigoodhart"]["passed"], true, "анти-Goodhart: {v}");
    let markers = v["antigoodhart"]["markers"].as_u64().unwrap_or(0);
    assert!(markers >= 12, "маркеров в пуле: {markers}");

    // Пороги (PREREGISTRATION.md §4). Порог в комментарии — как в задаче.
    // E1: повтор скрытой конвенции FI+L ≤ 5%, контроль FI = 100%.
    assert!(json_field(&v, "FI+L", "repeat_failure_rate") <= 0.05);
    assert!(json_field(&v, "FI", "repeat_failure_rate") > 0.99);
    // E1: первый PASS на T1 второй серии ≥ 80%.
    assert!(json_field(&v, "FI+L", "first_pass_t1_series2") >= 0.80);
    // E2: перенос на второго исполнителя ≥ 90%.
    assert!(json_field(&v, "X", "transfer_rate") >= 0.90);
    // E3: отложенное семейство ≥ 70%.
    assert!(json_field(&v, "GEN", "migration_compliance") >= 0.70);
    // E4: две конвенции одного kind ≥ 60%.
    assert!(json_field(&v, "PAIR", "first_pass_after_pair") >= 0.60);
    // E5: pre-commit на задачах 6–10 ≥ 50%.
    assert!(json_field(&v, "PROMO", "precommit_caught_rate") >= 0.50);
    // E6: повтор у поздно стартовавших ≤ 20%.
    assert!(json_field(&v, "FLEET", "late_repeat_rate") <= 0.20);
    // Прогонов исполнителя на задачу после урока ≤ 1.10.
    assert!(json_field(&v, "FI+L", "dispatches_per_task") <= 1.10);
}

/// Анти-Goodhart греп отдельной командой падает при нарушении и проходит чисто.
#[test]
fn antigoodhart_grep_clean() {
    if !python3_available() {
        eprintln!("skipped: no python3");
        return;
    }
    let (rc, out) = run(&["--check-antigoodhart"]);
    assert_eq!(rc, 0, "анти-Goodhart греп: {out}");
    assert!(out.contains("PASS"), "греп: {out}");
}
