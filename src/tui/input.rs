//! Состояние строки ввода TUI: текст, курсор, история, автодополнение
//! слэш-команд.

use std::collections::VecDeque;

use super::text;

/// Максимум записей в истории ввода.
const MAX_HISTORY: usize = 100;

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
    }

    /// Вставляет символ в позицию курсора.
    pub(crate) fn insert_char(&mut self, c: char) {
        self.completion = None;
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
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
