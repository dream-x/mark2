//! Поле ввода: многострочный буфер с курсором.
//!
//! Курсор — байтовая позиция в тексте, всегда на границе символа. Строки —
//! логические (по `\n`); перенос по ширине экрана считает отрисовка
//! ([`super::ui`]), здесь его нет.
//!
//! В буфере только печатные символы и `\n`: вставка ([`InputBuffer::insert_str`])
//! приводит переводы строк к `\n`, табуляцию — к [`super::wrap::TAB`], прочие
//! управляющие символы выбрасывает. Тогда ширина на экране однозначно
//! считается по символам, и курсор стоит там, где его видно.

use super::wrap::{TAB, char_width};

/// Текст поля ввода и позиция курсора.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InputBuffer {
    text: String,
    cursor: usize,
}

impl InputBuffer {
    /// Текущий текст.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Позиция курсора (байт).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Пусто ли поле.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Заменить текст целиком; курсор — в конец.
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    /// Забрать текст, оставив поле пустым.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    /// Вставить символ в позицию курсора.
    pub fn insert_char(&mut self, c: char) {
        if c == '\n' || !c.is_control() {
            self.text.insert(self.cursor, c);
            self.cursor += c.len_utf8();
        } else if c == '\t' {
            self.insert_clean(TAB);
        }
    }

    /// Вставить кусок текста (вставка из буфера обмена): `\r\n` и `\r` — в
    /// `\n`, табуляция — в пробелы, прочие управляющие символы — прочь.
    pub fn insert_str(&mut self, raw: &str) {
        let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
        let mut clean = String::with_capacity(normalized.len());
        for c in normalized.chars() {
            match c {
                '\n' => clean.push('\n'),
                '\t' => clean.push_str(TAB),
                c if c.is_control() => {}
                c => clean.push(c),
            }
        }
        self.insert_clean(&clean);
    }

    fn insert_clean(&mut self, clean: &str) {
        self.text.insert_str(self.cursor, clean);
        self.cursor += clean.len();
    }

    /// Стереть символ перед курсором.
    pub fn backspace(&mut self) {
        if let Some(previous) = self.prev_boundary() {
            self.text.replace_range(previous..self.cursor, "");
            self.cursor = previous;
        }
    }

    /// Стереть символ под курсором.
    pub fn delete(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.text.replace_range(self.cursor..next, "");
        }
    }

    /// Стереть от начала строки до курсора (Ctrl+U).
    pub fn kill_to_line_start(&mut self) {
        let start = self.line_start(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Курсор на символ влево.
    pub fn left(&mut self) {
        if let Some(previous) = self.prev_boundary() {
            self.cursor = previous;
        }
    }

    /// Курсор на символ вправо.
    pub fn right(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.cursor = next;
        }
    }

    /// Курсор в начало логической строки.
    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    /// Курсор в конец логической строки.
    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// Курсор на строку вверх, на ту же колонку (или в конец более короткой
    /// строки). Вернуть `false`, если выше строк нет.
    pub fn up(&mut self) -> bool {
        let start = self.line_start(self.cursor);
        if start == 0 {
            return false;
        }
        let column = self.column(start);
        let previous_start = self.line_start(start - 1);
        self.cursor = self.at_column(previous_start, column);
        true
    }

    /// Курсор на строку вниз. Вернуть `false`, если ниже строк нет.
    pub fn down(&mut self) -> bool {
        let end = self.line_end(self.cursor);
        if end == self.text.len() {
            return false;
        }
        let column = self.column(self.line_start(self.cursor));
        self.cursor = self.at_column(end + 1, column);
        true
    }

    /// Колонка курсора (в колонках экрана) от начала его строки.
    fn column(&self, line_start: usize) -> usize {
        self.text[line_start..self.cursor]
            .chars()
            .map(char_width)
            .sum()
    }

    /// Позиция в строке, начинающейся с `line_start`, ближайшая к колонке
    /// `column` слева.
    fn at_column(&self, line_start: usize, column: usize) -> usize {
        let line_end = self.line_end(line_start);
        let mut col = 0;
        for (offset, c) in self.text[line_start..line_end].char_indices() {
            let w = char_width(c);
            if col + w > column {
                return line_start + offset;
            }
            col += w;
        }
        line_end
    }

    fn line_start(&self, position: usize) -> usize {
        self.text[..position]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    fn line_end(&self, position: usize) -> usize {
        self.text[position..]
            .find('\n')
            .map_or(self.text.len(), |index| position + index)
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> InputBuffer {
        let mut input = InputBuffer::default();
        for c in text.chars() {
            input.insert_char(c);
        }
        input
    }

    #[test]
    fn typing_and_erasing_respect_char_boundaries() {
        let mut input = typed("привет");
        assert_eq!(input.text(), "привет");
        assert_eq!(input.cursor(), "привет".len());
        input.backspace();
        input.left();
        input.left();
        input.delete();
        assert_eq!(input.text(), "прие");
        input.insert_char('ё');
        assert_eq!(input.text(), "приёе");
        input.home();
        input.backspace();
        input.insert_char('>');
        input.end();
        input.right();
        input.insert_char('!');
        assert_eq!(input.text(), ">приёе!");
    }

    #[test]
    fn take_empties_the_field() {
        let mut input = typed("задача");
        assert_eq!(input.take(), "задача");
        assert!(input.is_empty());
        assert_eq!(input.cursor(), 0);
    }

    /// Вставка приводит переводы строк к `\n`, табы — к пробелам, прочие
    /// управляющие символы выбрасывает: в буфере только то, что видно.
    #[test]
    fn paste_is_normalized() {
        let mut input = InputBuffer::default();
        input.insert_str("a\r\nb\rc\td\u{1b}[31me\u{7}");
        assert_eq!(input.text(), "a\nb\nc    d[31me");
        assert_eq!(input.cursor(), input.text().len());

        let mut input = InputBuffer::default();
        input.insert_char('\t');
        input.insert_char('\u{1b}');
        assert_eq!(input.text(), TAB);
    }

    #[test]
    fn up_and_down_keep_the_column() {
        let mut input = typed("первая строка\nвт\nтретья строка");
        assert!(!input.down());
        assert!(input.up());
        // Вторая строка короче — курсор в её конце.
        assert_eq!(input.cursor(), "первая строка\nвт".len());
        assert!(input.up());
        // Колонка 2 («вт») — на первой строке это позиция после «пе».
        assert_eq!(input.cursor(), "пе".len());
        assert!(!input.up());
        input.home();
        input.right();
        assert!(input.down());
        assert_eq!(input.cursor(), "первая строка\nв".len());
    }

    #[test]
    fn kill_to_line_start_stops_at_the_newline() {
        let mut input = typed("одна\nдве три");
        input.left();
        input.kill_to_line_start();
        assert_eq!(input.text(), "одна\nи");
        assert_eq!(input.cursor(), "одна\n".len());
    }
}
