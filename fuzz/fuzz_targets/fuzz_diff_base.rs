#![no_main]
//! Фаззер разбора базы диффа (волна C1): произвольные байты → строка базы →
//! `normalize_base_range` + `base_rev` (формы `A`, `A..B`, `A...B`).
//! Инвариант: чистые строковые функции не паникуют ни на каком Unicode.

use arch_harness::control::{base_rev, normalize_base_range};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    // Сырая форма и нормализованная: обе дорожки разбора диапазона.
    let _ = base_rev(&text);
    let normalized = normalize_base_range(&text);
    let _ = base_rev(&normalized);
});
