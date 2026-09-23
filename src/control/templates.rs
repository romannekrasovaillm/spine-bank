//! Шаблоны документов контура: создание ADR по шаблону AI-DLC с очередным
//! номером (транслитерация кириллицы в слаг, ADR-002; разбор ID — общий regex
//! модели, ADR-003).

use std::path::{Path, PathBuf};

use crate::error::{HarnessError, Result};

/// Создаёт новый ADR по шаблону AI-DLC (Status/Context/Decision/Alternatives/
/// Consequences/Reversibility/References) с очередным номером в каталоге.
///
/// Номер — max(существующие `ADR-NNN-*`) + 1; каталог создаётся при
/// отсутствии. Имя файла: `ADR-NNN-kebab-case-title.md` (кириллица
/// транслитерируется, ADR-002). Разбор ID — общий regex модели
/// (`crate::model::id_re`, ADR-003), файлы не-ADR сущностей игнорируются.
///
/// # Errors
/// Каталог недоступен/не создаётся, файл уже существует.
pub fn adr_new(dir: &Path, title: &str) -> Result<PathBuf> {
    adr_new_with_author(dir, title, None)
}

/// То же с явной моделью-автором документа (J3, ADR-048): метка пишется в
/// шапку (`- Модель-автор: …`) и становится частью документа — судья берёт
/// автора оттуда, а не со слов в момент судейства. `human` / `human:<имя>` —
/// документ пишет человек.
///
/// # Errors
/// Как у [`adr_new`].
pub fn adr_new_with_author(dir: &Path, title: &str, author_model: Option<&str>) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| HarnessError::io(dir, e))?;
    let re_id = crate::model::id_re()
        .map_err(|e| HarnessError::Control(format!("внутренний regex ID: {e}")))?;
    let mut max_n = 0u64;
    let rd = std::fs::read_dir(dir).map_err(|e| HarnessError::io(dir, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| HarnessError::io(dir, e))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(caps) = re_id.captures(&name) {
            // Нумеруются только ADR-файлы; прочие сущности (CMP-*, AD-*) мимо.
            if caps.get(1).is_none_or(|m| m.as_str() != "ADR") {
                continue;
            }
            let Some(num) = caps.get(2) else {
                continue; // группа номера гарантирована паттерном; страховка от паники
            };
            let n: u64 = num.as_str().parse().map_err(|_| {
                HarnessError::Control(format!("некорректный номер ADR в имени файла '{name}'"))
            })?;
            max_n = max_n.max(n);
        }
    }
    let next = max_n + 1;
    let file = dir.join(format!("ADR-{next:03}-{}.md", kebab_slug(title)));
    if file.exists() {
        return Err(HarnessError::Control(format!(
            "ADR уже существует: {}",
            file.display()
        )));
    }
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    std::fs::write(&file, adr_template(next, title, &date, author_model))
        .map_err(|e| HarnessError::io(&file, e))?;
    Ok(file)
}

/// Практичная транслитерация кириллической буквы для слагов (ADR-002).
/// `Some("")` для ъ/ь (опускаются без дефиса), `None` для не-кириллицы.
fn translit_cyrillic(ch: char) -> Option<&'static str> {
    let lat = match ch {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' | 'э' => "e",
        'ё' => "yo",
        'ж' => "zh",
        'з' => "z",
        'и' => "i",
        'й' | 'ы' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "h",
        'ц' => "c",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "sch",
        'ъ' | 'ь' => "",
        'ю' => "yu",
        'я' => "ya",
        _ => return None,
    };
    Some(lat)
}

/// kebab-case slug заголовка: ASCII-буквы/цифры в нижний регистр, кириллица
/// транслитерируется (`translit_cyrillic`), всё прочее — в `-`, повторы `-`
/// схлопываются. Пустой результат (пустой/символьный заголовок) → `"adr"`.
pub(crate) fn kebab_slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut dash = true; // подавляет '-' в начале
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if let Some(lat) = translit_cyrillic(ch) {
            out.push_str(lat);
            if !lat.is_empty() {
                dash = false;
            }
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "adr".into()
    } else {
        trimmed.to_string()
    }
}

/// Шаблон ADR по AI-DLC с placeholder-комментариями.
fn adr_template(n: u64, title: &str, date: &str, author_model: Option<&str>) -> String {
    // Метка автора — часть шапки: по ней судья отличает автора от судьи
    // (J3, ADR-048). Не задана — строки нет: выдумывать автора нельзя.
    let author =
        author_model.map_or_else(String::new, |author| format!("- Модель-автор: {author}\n"));
    format!(
        "# ADR-{n:03}. {title}\n\
        \n\
        - Date: {date}\n\
        - Status: Proposed\n\
        {author}\
        \n\
        ## Context\n\
        \n\
        <!-- Что заставляет принять решение: контекст, силы, ограничения. -->\n\
        \n\
        ## Decision\n\
        \n\
        <!-- Принятое решение: одно, явно сформулированное. -->\n\
        \n\
        ## Alternatives Considered\n\
        \n\
        | Вариант | Плюсы | Минусы |\n\
        |---------|-------|--------|\n\
        | <!-- вариант --> | <!-- плюсы --> | <!-- минусы --> |\n\
        \n\
        ## Consequences\n\
        \n\
        ### Positive\n\
        \n\
        <!-- Что станет лучше. -->\n\
        \n\
        ### Negative\n\
        \n\
        <!-- Цена решения: что станет хуже, какие риски принимаем. Обязательно к заполнению. -->\n\
        \n\
        ## Reversibility\n\
        \n\
        <!-- Обратимость: reversible | costly | irreversible. Обоснование оценки. -->\n\
        \n\
        ## References\n\
        \n\
        <!-- Ссылки на spine (AD-n), спеки, обсуждения. -->\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adr_new_numbers_sequentially() {
        let dir = tempfile::tempdir().unwrap();
        let adr_dir = dir.path().join("docs/adr"); // каталога ещё нет — должен создаться
        let first = adr_new(&adr_dir, "API Contract Change").unwrap();
        assert_eq!(
            first.file_name().unwrap().to_string_lossy(),
            "ADR-001-api-contract-change.md"
        );
        let content = std::fs::read_to_string(&first).unwrap();
        for needle in [
            "# ADR-001. API Contract Change",
            "- Status: Proposed",
            "## Context",
            "## Decision",
            "## Alternatives Considered",
            "### Positive",
            "### Negative",
            "## Reversibility",
            "reversible | costly | irreversible",
            "## References",
            "<!--",
        ] {
            assert!(content.contains(needle), "в шаблоне нет '{needle}'");
        }
        let second = adr_new(&adr_dir, "Шина событий").unwrap();
        assert_eq!(
            second.file_name().unwrap().to_string_lossy(),
            "ADR-002-shina-sobytiy.md",
            "кириллица транслитерируется (ADR-002)"
        );
    }

    #[test]
    fn adr_new_ignores_non_adr_entity_files() {
        // Общий regex ID (model::id_re) матчит и CMP-*/AD-* имена —
        // нумерацию ADR они затрагивать не должны (ADR-003).
        let dir = tempfile::tempdir().unwrap();
        let adr_dir = dir.path().join("docs/adr");
        std::fs::create_dir_all(&adr_dir).unwrap();
        for name in [
            "CMP-999-payment-gateway.md",
            "AD-27-multi-currency.md",
            "README.md",
        ] {
            std::fs::write(adr_dir.join(name), "посторонний файл").unwrap();
        }
        let first = adr_new(&adr_dir, "Outbox").unwrap();
        assert_eq!(
            first.file_name().unwrap().to_string_lossy(),
            "ADR-001-outbox.md",
            "CMP-999/AD-27 не влияют на номер ADR"
        );
    }

    #[test]
    fn kebab_slug_transliterates_cyrillic() {
        assert_eq!(
            kebab_slug("Сегментация доверенных зон (4 зоны)"),
            "segmentaciya-doverennyh-zon-4-zony"
        );
        assert_eq!(
            kebab_slug("Стратегия идемпотентности на точках входа"),
            "strategiya-idempotentnosti-na-tochkah-vhoda"
        );
        // ъ/ь опускаются без дефиса, ё → yo, щ → sch
        assert_eq!(kebab_slug("Подъём щёточный"), "podyom-schyotochnyy");
    }

    #[test]
    fn kebab_slug_mixed_latin_cyrillic() {
        assert_eq!(kebab_slug("Outbox паттерн"), "outbox-pattern");
        assert_eq!(kebab_slug("ADR для async рельсов"), "adr-dlya-async-relsov");
    }

    #[test]
    fn kebab_slug_empty_falls_back_to_adr() {
        assert_eq!(kebab_slug(""), "adr");
        assert_eq!(kebab_slug("!!! ..."), "adr");
    }
}
