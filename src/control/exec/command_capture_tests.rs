//! Тесты захвата вывода `command_succeeds` (хвост вывода, редакция секретов).
//! Вынесены из `exec.rs` чистым перемещением (граница C-33).

use super::runner::*;
use std::time::Duration;

#[test]
fn run_with_timeout_captures_output_tail_on_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outcome = run_with_timeout(
        tmp.path(),
        "echo marker-строка-вывода; echo ошибка-в-стдошибку >&2; exit 3",
        Duration::from_secs(10),
    )
    .expect("прогон команды");
    assert_eq!(outcome.status.and_then(|s| s.code()), Some(3));
    assert!(outcome.tail.contains("marker-строка-вывода"));
    assert!(outcome.tail.contains("ошибка-в-стдошибку"));
}

#[test]
fn report_tail_keeps_last_lines_and_redacts_secrets() {
    use std::fmt::Write as _;
    let mut raw = String::new();
    for i in 1..=20 {
        writeln!(raw, "строка {i}").expect("запись в String не падает");
    }
    raw.push_str("ключ DEEPSEEK_API_KEY=sk-0123456789abcdef0123456789 в тексте\n");
    let tail = report_tail(&raw);
    assert_eq!(tail.lines().count(), REPORT_TAIL_LINES);
    assert!(tail.contains("строка 20"));
    assert!(!tail.contains("строка 5"), "старые строки обрезаны: {tail}");
    assert!(
        !tail.contains("sk-0123456789abcdef0123456789"),
        "секрет обязан быть замаскирован: {tail}"
    );
}
