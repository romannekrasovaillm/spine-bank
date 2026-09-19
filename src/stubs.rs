//! Детектор заглушек в артефактах архитектурного пакета: незаполненные места
//! шаблона (`<…>`) и явные «доделать» (`TODO`, `TBD`).
//!
//! Модуль общий для двух потребителей, у которых правило обязано совпадать:
//! - [`crate::delta::validate`] — «дельта из шаблона, а не из работы»;
//! - [`crate::evidence::verify`] — «бандл собран, но артефакты пустые»
//!   (находка 0.3.4: зелёный вердикт удостоверял наличие файла, а не его
//!   содержание).
//!
//! Два уровня строгости намеренно разные:
//! - [`is_template_stub`] — историческое правило `delta validate` (строка
//!   целиком `<…>` либо `TODO`/`TBD`); поведение 0.3.3 сохраняется
//!   байт-в-байт;
//! - [`is_stub_line`] — то же плюс `<…>`-вставка внутри строки (шаблоны вида
//!   `- **choice**: <выбор>`); используется бандлом, где цена пропуска выше
//!   цены ложного срабатывания.

/// Явные маркеры «доделать» в любом месте строки.
const STUB_WORDS: [&str; 2] = ["TODO", "TBD"];

/// Историческое правило `delta validate`: строка — незаполненное место
/// шаблона целиком (`<…>`) либо содержит `TODO`/`TBD`.
///
/// Правило заморожено: `delta validate` не должен менять вердикт от 0.3.3.
#[must_use]
pub fn is_template_stub(line: &str) -> bool {
    let t = line.trim();
    (t.starts_with('<') && t.ends_with('>')) || STUB_WORDS.iter().any(|m| t.contains(m))
}

/// Признак заглушки для бандла: [`is_template_stub`] плюс `<…>`-вставка
/// внутри строки.
///
/// Вставкой считается группа `<тело>` без `/`, `<`, `>` внутри, тело которой
/// непусто и содержит признак «человеческого текста»: пробел, точку, дефис,
/// подчёркивание или не-ASCII символ. Так `<выбор>`, `<...>` и `<CHANGE-ID>`
/// ловятся, а HTML-теги (`<br>`, `<strong>`) — нет.
#[must_use]
pub fn is_stub_line(line: &str) -> bool {
    is_template_stub(line) || has_angle_placeholder(line)
}

/// Есть ли в строке `<…>`-вставка-заглушка (см. [`is_stub_line`]).
#[must_use]
pub fn has_angle_placeholder(line: &str) -> bool {
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            return false;
        };
        let body = &after[..close];
        if looks_like_placeholder(body) {
            return true;
        }
        rest = &after[close + 1..];
    }
    false
}

/// Тело `<…>` похоже на незаполненное место, а не на HTML-тег.
fn looks_like_placeholder(body: &str) -> bool {
    !body.is_empty()
        && !body.contains('/')
        && (body.contains(' ')
            || body.contains('.')
            || body.contains('-')
            || body.contains('_')
            || !body.is_ascii())
}

/// Первая строка-заглушка текста: `(номер строки 1-based, фрагмент до 60
/// символов)`. `None` — заглушек нет.
///
/// `strict` выбирает уровень: `true` — [`is_stub_line`] (бандл), `false` —
/// [`is_template_stub`] (дельта).
#[must_use]
pub fn find_stub(text: &str, strict: bool) -> Option<(usize, String)> {
    let probe = if strict {
        is_stub_line
    } else {
        is_template_stub
    };
    text.lines()
        .enumerate()
        .find(|(_, l)| probe(l))
        .map(|(n, l)| (n + 1, l.trim().chars().take(60).collect::<String>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_stub_matches_frozen_delta_rule() {
        assert!(is_template_stub("<что и зачем меняем>"));
        assert!(is_template_stub("  <проверяемый критерий>  "));
        assert!(is_template_stub("TODO"));
        assert!(is_template_stub("здесь TBD позже"));
        // Вставка внутри строки старым правилом не ловится — и не должна:
        // вердикт `delta validate` от 0.3.3 не меняется.
        assert!(!is_template_stub("- **choice**: <выбор>"));
        assert!(!is_template_stub("## Проблема"));
    }

    #[test]
    fn strict_rule_catches_inline_placeholders() {
        assert!(is_stub_line("- **choice**: <выбор>"));
        assert!(is_stub_line("<...>"));
        assert!(is_stub_line("пункт <CHANGE-ID> не заполнен"));
        assert!(is_stub_line("<новые требования с критериями EARS>"));
    }

    #[test]
    fn strict_rule_does_not_flag_html_tags() {
        // `<br>` отдельной строкой ловит ЗАМОРОЖЕННОЕ правило (строка целиком
        // `<…>`) — поведение 0.3.3 сохраняется и здесь не оспаривается.
        assert!(is_template_stub("<br>"));
        // А вставки HTML внутри строки — не заглушки.
        assert!(!is_stub_line("текст <strong>важное</strong> дальше"));
        assert!(!is_stub_line("см. <https://example.org/a>"));
        assert!(!has_angle_placeholder("</code>"));
        assert!(!is_stub_line("## Решение"));
    }

    #[test]
    fn find_stub_reports_line_number_and_fragment() {
        let text = "## Решение\n\nОк.\n\n- **choice**: <выбор>\n";
        let (n, frag) = find_stub(text, true).expect("заглушка");
        assert_eq!(n, 5);
        assert!(frag.contains("<выбор>"), "{frag}");
        assert!(find_stub("чистый текст без заглушек", true).is_none());
    }

    #[test]
    fn empty_line_is_not_a_stub_but_blank_body_is() {
        assert!(!is_stub_line(""));
        // Пробельное тело — всё ещё незаполненное место.
        assert!(is_stub_line("<   >"));
    }
}
