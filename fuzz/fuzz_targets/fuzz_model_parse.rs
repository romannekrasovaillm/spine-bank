#![no_main]
//! Фаззер разбора сущностей модели (волна C1): произвольные байты →
//! markdown с YAML-frontmatter → `split_frontmatter` + `parse_entity`.
//! Инвариант: любой вход завершается `Ok`/`Err`, без паник и зависаний.

use std::path::Path;

use arch_harness::model::{parse_entity, split_frontmatter};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    // Сплиттер frontmatter — входная точка parse_entity, дёргаем отдельно.
    let _ = split_frontmatter(&text);
    // Путь не читается с диска — используется только в сообщениях об ошибках
    // и в поле `file` разобранной сущности.
    let _ = parse_entity(Path::new("FUZZ-000.md"), &text);
});
