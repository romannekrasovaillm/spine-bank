//! Состояние строки ввода TUI: текст, курсор, история, автодополнение
//! слэш-команд.

use std::collections::VecDeque;

use super::text;

/// Максимум записей в истории ввода.
const MAX_HISTORY: usize = 100;

/// Максимум символов, удерживаемых как возможный обрывок SGR-репорта мыши.
const MOUSE_HELD_MAX: usize = 32;

/// Сколько символов после распознанного репорта считаем его хвостом.
const MOUSE_BURST_CHARS: usize = 32;

/// Фильтр терминального мусора в текстовом вводе.
///
/// В режиме mouse capture терминал шлёт SGR-репорты (`ESC [ < Cb ; Cx ; Cy M`).
/// Crossterm разбирает их в `Event::Mouse`, но при разрыве escape-последовательности
/// по границе чтения ESC теряется, и остаток (`[<35;152;1M`) приходит обычными
/// `KeyCode::Char`: попадает в поле ввода и уходит модели вместе с текстом
/// (инцидент 2026-10-17, banking edition). Фильтр удерживает короткие
/// «репортные» хвосты и отбрасывает их, не трогая обычный текст.
#[derive(Debug, Default)]
struct MouseReportFilter {
    /// Удерживаемый кандидат в обрывок репорта.
    held: String,
    /// Сколько ещё символов после репорта считаем его хвостом.
    burst: usize,
}

impl MouseReportFilter {
    /// Принимает символ; всё, что признано обычным текстом, дописывает в `out`.
    fn feed(&mut self, c: char, out: &mut String) {
        if !is_report_char(c) {
            self.finish(out);
            out.push(c);
            return;
        }
        // Вне потока репортов цифра и концевой `M` — обычный текст: держать
        // их до следующего символа значит «прятать» только что набранное
        // число. Обрывок вне потока начинается со структурного маркера.
        if self.held.is_empty() && self.burst == 0 && (c.is_ascii_digit() || matches!(c, 'M' | 'm'))
        {
            out.push(c);
            return;
        }
        self.held.push(c);
        if self.held.len() > MOUSE_HELD_MAX {
            self.finish(out);
            return;
        }
        if self.is_complete_report() {
            self.held.clear();
            self.burst = MOUSE_BURST_CHARS;
            return;
        }
        if self.burst > 0 {
            if self.held.ends_with(['M', 'm']) {
                self.held.clear();
                self.burst = MOUSE_BURST_CHARS;
                return;
            }
            self.burst -= 1;
        }
        if !could_be_report_prefix(&self.held) {
            self.finish(out);
        }
    }

    /// Сбрасывает удержанное в текст. Внутри потока репортов недописанный
    /// обрывок (`[<35;13`) отбрасывается, а не попадает в поле ввода.
    fn finish(&mut self, out: &mut String) {
        if self.burst > 0 && looks_like_report_debris(&self.held) {
            self.held.clear();
        } else {
            out.push_str(&self.held);
            self.held.clear();
        }
        self.burst = 0;
    }

    /// Завершённый репорт: `Cb;Cx;Cy` + `M`/`m`, с префиксом `[<`/`<`
    /// или без него (усечённые формы `;Cx;CyM`, `Cx;CyM`).
    fn is_complete_report(&self) -> bool {
        let (body, bracketed) = if let Some(rest) = self.held.strip_prefix("[<") {
            (rest, true)
        } else if let Some(rest) = self.held.strip_prefix('<') {
            (rest, true)
        } else {
            (self.held.as_str(), false)
        };
        let Some(body) = body.strip_suffix(['M', 'm']) else {
            return false;
        };
        if body.is_empty() {
            return self.burst > 0;
        }
        let body = body.strip_prefix(';').unwrap_or(body);
        let mut parts = 0_usize;
        for part in body.split(';') {
            if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            parts += 1;
        }
        parts <= 3 && (bracketed || self.held.contains(';') || self.burst > 0)
    }
}

/// Символ, из которых состоят SGR-репорты мыши.
fn is_report_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '[' | '<' | ';' | 'M' | 'm')
}

/// Может ли удержанная строка оказаться началом/продолжением репорта.
fn could_be_report_prefix(s: &str) -> bool {
    if s.is_empty() || s.ends_with(['M', 'm']) {
        return false;
    }
    if s == "[" || s == "<" {
        return true;
    }
    let body = s
        .strip_prefix("[<")
        .or_else(|| s.strip_prefix('<'))
        .unwrap_or(s);
    body.chars().all(|c| c.is_ascii_digit() || c == ';')
}

/// Похоже ли удержанное на обрывок репорта (а не на начало обычного текста).
fn looks_like_report_debris(s: &str) -> bool {
    !s.is_empty() && (s.starts_with('[') || s.starts_with('<') || s.contains(';'))
}

/// Состояние строки ввода: текст, курсор, история, автодополнение.
#[derive(Debug, Default)]
pub(crate) struct InputState {
    /// Текст ввода.
    text: String,
    /// Позиция курсора (байтовый индекс, всегда на границе char).
    cursor: usize,
    /// История отправленных строк (старые — в начале).
    pub(crate) history: VecDeque<String>,
    /// Индекс навигации по истории (None — редактируется черновик).
    hist_idx: Option<usize>,
    /// Черновик, сохранённый при уходе в историю.
    draft: String,
    /// Активные кандидаты автодополнения и текущий индекс (цикл по Tab).
    completion: Option<(Vec<&'static str>, usize)>,
    /// Фильтр обрывков мышиных репортов (инцидент 2026-10-17).
    filter: MouseReportFilter,
}

impl InputState {
    /// Текущий текст.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Позиция курсора (байтовый индекс).
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Устанавливает текст, курсор — в конец; сбрасывает автодополнение.
    pub(crate) fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
        self.completion = None;
        self.filter = MouseReportFilter::default();
    }

    /// Вставляет символ в позицию курсора, отбрасывая обрывки мышиных
    /// репортов (см. [`MouseReportFilter`]).
    pub(crate) fn insert_char(&mut self, c: char) {
        self.completion = None;
        let mut commit = String::new();
        self.filter.feed(c, &mut commit);
        if commit.is_empty() {
            return;
        }
        self.text.insert_str(self.cursor, &commit);
        self.cursor += commit.len();
    }

    /// Отдаёт в текст удержанный хвост. Вызывается перед не-символьными
    /// клавишами (Enter, стрелки), чтобы отложенный ввод не терялся.
    pub(crate) fn flush_debris(&mut self) {
        let mut commit = String::new();
        self.filter.finish(&mut commit);
        if commit.is_empty() {
            return;
        }
        self.text.insert_str(self.cursor, &commit);
        self.cursor += commit.len();
    }

    /// Вставляет перевод строки (многострочный ввод: Ctrl+J / Shift+Enter).
    pub(crate) fn insert_newline(&mut self) {
        self.completion = None;
        self.text.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    /// Логическая строка (по '\n') и колонка в символах под курсором.
    fn line_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let line = before.matches('\n').count();
        let col = before.rsplit('\n').next().map_or(0, |s| s.chars().count());
        (line, col)
    }

    /// Байтовый индекс колонки `col` логической строки `line`
    /// (col клампится по длине строки).
    fn byte_at_line_col(&self, line: usize, col: usize) -> usize {
        let mut start = 0;
        for (n, part) in self.text.split('\n').enumerate() {
            if n == line {
                return part
                    .char_indices()
                    .nth(col)
                    .map_or(start + part.len(), |(i, _)| start + i);
            }
            start += part.len() + 1; // + '\n'
        }
        self.text.len()
    }

    /// Up внутри многострочного ввода: true, если курсор ушёл на строку выше
    /// (false — курсор на первой строке, Up свободен для истории).
    pub(crate) fn move_up_line(&mut self) -> bool {
        let (line, col) = self.line_col();
        if line == 0 {
            return false;
        }
        self.cursor = self.byte_at_line_col(line - 1, col);
        true
    }

    /// Down внутри многострочного ввода: true, если курсор ушёл на строку
    /// ниже (false — курсор на последней строке, Down свободен для истории).
    pub(crate) fn move_down_line(&mut self) -> bool {
        let (line, col) = self.line_col();
        if line >= self.text.matches('\n').count() {
            return false;
        }
        self.cursor = self.byte_at_line_col(line + 1, col);
        true
    }

    /// Удаляет символ перед курсором.
    pub(crate) fn backspace(&mut self) {
        self.completion = None;
        if self.cursor == 0 {
            return;
        }
        let prev = self.text[..self.cursor]
            .chars()
            .next_back()
            .map_or(1, char::len_utf8);
        self.text.replace_range(self.cursor - prev..self.cursor, "");
        self.cursor -= prev;
    }

    /// Удаляет символ под курсором.
    pub(crate) fn delete(&mut self) {
        self.completion = None;
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.text
                .replace_range(self.cursor..self.cursor + c.len_utf8(), "");
        }
    }

    /// Курсор на символ влево.
    pub(crate) fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.text[..self.cursor]
                .chars()
                .next_back()
                .map_or(0, |c| self.cursor - c.len_utf8());
        }
    }

    /// Курсор на символ вправо.
    pub(crate) fn move_right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    /// Курсор в начало текущей логической строки (по '\n').
    pub(crate) fn move_home(&mut self) {
        let (line, _) = self.line_col();
        self.cursor = self.byte_at_line_col(line, 0);
    }

    /// Курсор в конец текущей логической строки (по '\n').
    pub(crate) fn move_end(&mut self) {
        let (line, _) = self.line_col();
        let len = self
            .text
            .split('\n')
            .nth(line)
            .map_or(0, |s| s.chars().count());
        self.cursor = self.byte_at_line_col(line, len);
    }

    /// Забирает введённую строку, сохраняя непустую в истории.
    pub(crate) fn submit(&mut self) -> String {
        let text = self.text.trim().to_string();
        if !text.is_empty() && self.history.back() != Some(&text) {
            self.history.push_back(text.clone());
            while self.history.len() > MAX_HISTORY {
                self.history.pop_front();
            }
        }
        self.text.clear();
        self.cursor = 0;
        self.hist_idx = None;
        self.draft.clear();
        self.completion = None;
        self.filter = MouseReportFilter::default();
        text
    }

    /// Up: шаг назад по истории (черновик сохраняется).
    pub(crate) fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.hist_idx {
            None => {
                self.draft.clone_from(&self.text);
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.hist_idx = Some(idx);
        if let Some(entry) = self.history.get(idx) {
            self.set_text(entry.clone());
        }
    }

    /// Down: шаг вперёд по истории; за последней записью — черновик.
    pub(crate) fn history_down(&mut self) {
        let Some(idx) = self.hist_idx else {
            return;
        };
        if idx + 1 < self.history.len() {
            self.hist_idx = Some(idx + 1);
            if let Some(entry) = self.history.get(idx + 1) {
                self.set_text(entry.clone());
            }
        } else {
            self.hist_idx = None;
            let draft = std::mem::take(&mut self.draft);
            self.set_text(draft);
        }
    }

    /// Tab: дополняет слэш-команду; повторные Tab циклят кандидатов.
    /// Возвращает false, если кандидатов нет (Tab свободен для вкладок).
    pub(crate) fn complete_tab(&mut self) -> bool {
        // Уже в цикле дополнения и текст не редактировали — следующий кандидат.
        if let Some((cands, idx)) = &mut self.completion {
            if self.text == cands[*idx] {
                *idx = (*idx + 1) % cands.len();
                let candidate = cands[*idx];
                // Напрямую, не через set_text: цикл кандидатов сохраняем.
                self.text = candidate.to_string();
                self.cursor = self.text.len();
                return true;
            }
        }
        self.completion = None;
        let cands = text::completion_candidates(&self.text);
        if cands.is_empty() {
            return false;
        }
        self.set_text(cands[0].to_string());
        self.completion = Some((cands, 0));
        true
    }

    /// Приглушённая подсказка-дополнение справа от ввода (суффикс кандидата).
    pub(crate) fn ghost_hint(&self) -> Option<String> {
        let first = text::completion_candidates(&self.text).into_iter().next()?;
        Some(first[self.text.len()..].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_chars(input: &mut InputState, s: &str) {
        for c in s.chars() {
            input.insert_char(c);
        }
    }

    #[test]
    fn mouse_report_fragments_never_become_input_text() {
        // Инцидент 2026-10-17: обрывки SGR-репортов мыши попали в поле
        // ввода как символы и были отправлены модели вместе с текстом.
        let mut input = InputState::default();
        type_chars(
            &mut input,
            ";1M[<35;152;1M137;2M5;124;3M[<35;114;4M[<35;113;4MM;113;4M\
             [<35;112;4M[<35;112;4M[<35;112;4M[<35;113;4M[<35;113;4M\
             [<35;114;4M[<35;115;1M116;1M[<35;117;1M5;119;1M[<35;119;1M\
             35;121;1M[<35;124;1M[<35;126;1M35;139;10M[<35;138;10M\
             5;131;13M;120;17M5M[<35;103;14M5;138;6M<35;152;2M\
             <35;156;1M[<35;141;11M5;81;23M[<35;96;22M[<35;92;17M\
             35;89;16M16M[<35;87;16M[<35;88;15M5;88;12M;90;10M\
             [<35;120;2M[<35;131;1M[<35;153;2M140;7M30;10M[<35;118;12M\
             [<35;90;13M[<35;88;14M14M[<35;89;13M[<35;93;11M\
             35;106;2M[<35;159;1M[<35;124;9M[<35;102;16M<35;87;20M\
             ;21M7;21M[<35;90;21M<35;94;22M;24M[<35;109;24M\
             [<35;111;26M5;111;26M[<35;109;27MM[<35;109;26M\
             ;109;26M10;25M[<35;110;24M11;22M35;116;17M[<35;129;7M\
             ;133;3M35;134;3M[<35;134;3M[<35;13",
        );
        type_chars(&mut input, "малый срез");
        assert_eq!(input.text(), "малый срез", "мышиный мусор в поле ввода");
    }

    #[test]
    fn ordinary_text_with_digits_and_semicolons_survives() {
        let mut input = InputState::default();
        let text = "версия 0.4.15; тест 2+2, шаг 35";
        type_chars(&mut input, text);
        assert_eq!(input.text(), text);
    }

    #[test]
    fn lone_report_like_token_is_text_not_mouse_debris() {
        let mut input = InputState::default();
        type_chars(&mut input, "5M");
        assert_eq!(input.text(), "5M");
    }

    #[test]
    fn flush_debris_keeps_ordinary_held_text() {
        let mut input = InputState::default();
        type_chars(&mut input, "35;");
        input.flush_debris();
        assert_eq!(input.text(), "35;");
    }

    #[test]
    fn flush_debris_drops_unfinished_report_in_burst() {
        let mut input = InputState::default();
        type_chars(&mut input, ";1M[<35;13");
        input.flush_debris();
        assert_eq!(input.text(), "", "недописанный обрывок не должен попасть в текст");
    }
}
