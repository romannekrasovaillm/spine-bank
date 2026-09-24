#![no_main]
//! Фаззер диффа контрактов (волна C1): произвольные байты → пара файлов
//! контрактов со случайным расширением → `detect_format` + `diff_report`.
//! Инвариант: любой вход завершается `Ok`/`Err`, без паник и зависаний.
//!
//! Формат входа: первый байт выбирает расширение обоих файлов, остальное
//! делится по первому NUL на «старую» и «новую» версии документа.

use std::path::PathBuf;

use arch_harness::contract_diff::{detect_format, diff_report};
use libfuzzer_sys::fuzz_target;

/// Расширения, по которым детектор идёт «короткой дорогой» (proto/avsc/sql),
/// плюс нейтральные, где формат выводится из содержимого.
const EXTENSIONS: [&str; 6] = ["yaml", "json", "proto", "avsc", "sql", "txt"];

fuzz_target!(|data: &[u8]| {
    let Some((&ext_idx, rest)) = data.split_first() else {
        return;
    };
    let ext = EXTENSIONS[ext_idx as usize % EXTENSIONS.len()];
    let (old, new) = match rest.iter().position(|&b| b == 0) {
        Some(pos) => (&rest[..pos], &rest[pos + 1..]),
        None => (rest, &[][..]),
    };
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let old_path: PathBuf = dir.path().join(format!("old.{ext}"));
    let new_path: PathBuf = dir.path().join(format!("new.{ext}"));
    if std::fs::write(&old_path, old).is_err() || std::fs::write(&new_path, new).is_err() {
        return;
    }
    // Детектор формата отдельно: ему нужны и путь (расширение), и содержимое.
    let old_text = String::from_utf8_lossy(old);
    let _ = detect_format(&old_path, &old_text);
    // Полный дифф пары: авто-детект обоих файлов, без связки с моделью.
    let _ = diff_report(&old_path, &new_path, None, None);
});
