#![no_main]
//! Фаззер чтения `CONSTRAINTS.yaml` (волна C1): произвольные байты → файл →
//! `load_fitness_rules_with_skips` (разбор YAML, известные и «чужие» типы
//! правил, карточки полей). Инвариант: `Ok`/`Err`, без паник и зависаний.

use arch_harness::control::load_fitness_rules_with_skips;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("CONSTRAINTS.yaml");
    if std::fs::write(&path, data).is_err() {
        return;
    }
    // Вариант со скипами покрывает и неизвестные типы правил (E8) —
    // строго шире, чем load_fitness_rules.
    let _ = load_fitness_rules_with_skips(&path);
});
