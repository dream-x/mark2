//! Текст для экрана: санитария и перенос по ширине терминала.
//!
//! Перенос свой, а не `Paragraph::wrap` ratatui, по двум причинам: ленте
//! нужно знать число строк заранее (прокрутка считается в строках экрана), а
//! полю ввода — где на экране стоит курсор. И то и другое считается одной и
//! той же функцией [`wrap_ranges`], так что расчёт и отрисовка не
//! расходятся.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

use crate::render::terminal_safe;

/// Сколько пробелов рисуется вместо табуляции: ratatui управляющие символы
/// (и `\t` в их числе) молча выбрасывает.
pub const TAB: &str = "    ";

/// Текст события в виде, безопасном для экрана.
///
/// `\r\n` → `\n`; прочие управляющие символы, bidi- и zero-width-символы
/// экранируются тем же [`terminal_safe`], что и запрос подтверждения (ESC
/// или `\r` из ответа модели не доходят до терминала сырыми и не прячутся);
/// табуляция — [`TAB`].
pub fn display_text(raw: &str) -> String {
    terminal_safe(&raw.replace("\r\n", "\n")).replace('\t', TAB)
}

/// Ширина символа в колонках терминала; у управляющих — ноль.
pub fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Ширина строки в колонках терминала.
pub fn str_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// Перенос одной логической строки (без `\n`) по `width` колонкам: байтовые
/// диапазоны строк экрана, подряд и без пропусков.
///
/// Переносим по словам: строка экрана рвётся после последнего пробела, а
/// если пробела нет — посреди слова. Пробел, на котором порвали, остаётся в
/// конце предыдущей строки, поэтому диапазоны смыкаются: позиция курсора
/// однозначно попадает в одну из строк. Ни одна строка не шире `width`
/// (кроме вырожденного случая, когда один символ шире всей ширины).
pub fn wrap_ranges(line: &str, width: usize) -> Vec<Range<usize>> {
    if width == 0 || line.is_empty() {
        let whole = 0..line.len();
        return vec![whole];
    }
    let mut rows = Vec::new();
    let mut start = 0;
    let mut col = 0;
    // Байт сразу за последним пробелом текущей строки экрана.
    let mut last_break: Option<usize> = None;
    for (index, c) in line.char_indices() {
        let w = char_width(c);
        while col + w > width && col > 0 {
            let cut = match last_break {
                Some(cut) if cut > start => cut,
                _ => index,
            };
            rows.push(start..cut);
            start = cut;
            col = str_width(&line[start..index]);
            last_break = None;
        }
        col += w;
        if c == ' ' {
            last_break = Some(index + c.len_utf8());
        }
    }
    rows.push(start..line.len());
    rows
}

/// Перенос многострочного текста: строки экрана по порядку.
pub fn wrap_text(text: &str, width: usize) -> Vec<&str> {
    text.split('\n')
        .flat_map(|line| {
            wrap_ranges(line, width)
                .into_iter()
                .map(move |range| &line[range])
        })
        .collect()
}

/// Обрезать строку до `width` колонок с `…` на конце, если не влезла.
pub fn truncate(text: &str, width: usize) -> String {
    if str_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut col = 0;
    for c in text.chars() {
        let w = char_width(c);
        if col + w + 1 > width {
            break;
        }
        out.push(c);
        col += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(line: &str, width: usize) -> Vec<&str> {
        wrap_ranges(line, width)
            .into_iter()
            .map(|range| &line[range])
            .collect()
    }

    #[test]
    fn short_and_empty_lines_are_one_row() {
        assert_eq!(rows("", 10), [""]);
        assert_eq!(rows("привет", 10), ["привет"]);
        assert_eq!(rows("ровно10сим", 10), ["ровно10сим"]);
    }

    #[test]
    fn words_wrap_at_the_last_space_and_ranges_are_contiguous() {
        let line = "один два три четыре";
        assert_eq!(rows(line, 9), ["один два ", "три ", "четыре"]);
        let ranges = wrap_ranges(line, 9);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        assert_eq!(ranges.last().unwrap().end, line.len());
    }

    #[test]
    fn long_words_are_cut_and_no_row_is_wider_than_width() {
        assert_eq!(rows("абвгдежзик", 4), ["абвг", "дежз", "ик"]);
        let text = "x ".repeat(50) + &"длинноеслово".repeat(5);
        for row in rows(&text, 7) {
            assert!(str_width(row) <= 7, "{row:?}");
        }
    }

    /// Двухколоночные символы не разрезаются и не вылезают за край.
    #[test]
    fn wide_characters_count_as_two_columns() {
        assert_eq!(rows("漢字漢字", 5), ["漢字", "漢字"]);
        assert_eq!(str_width("漢字"), 4);
    }

    #[test]
    fn display_text_escapes_controls_and_expands_tabs() {
        assert_eq!(display_text("a\r\nb"), "a\nb");
        assert_eq!(display_text("a\tb"), "a    b");
        assert_eq!(display_text("x\u{1b}[2Ky\rz"), "x\\u{1b}[2Ky\\rz");
        assert_eq!(display_text("\u{202E}gnp"), "\\u{202e}gnp");
    }

    #[test]
    fn wrap_text_keeps_empty_lines() {
        assert_eq!(wrap_text("ab\n\ncd", 10), ["ab", "", "cd"]);
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("короткая", 20), "короткая");
        assert_eq!(truncate("очень длинная строка", 8), "очень д…");
        assert_eq!(str_width(&truncate("очень длинная строка", 8)), 8);
    }
}
