//! HTML → читаемый текст для `web_fetch`: выжимка страницы, а не рендер.
//!
//! Свой минимальный стриппер вместо `html2text`: тот тянет html5ever со
//! string_cache/phf/tendril и build-скриптами, а модели нужен текст без
//! разметки, а не точная вёрстка. Что делается:
//!
//! * `<title>` выдёргивается отдельно (первый, вне `<svg>`);
//! * содержимое `script`, `style`, `noscript`, `template`, `svg`, `nav`,
//!   `select` и подобных выбрасывается целиком — это не текст страницы;
//! * блочные теги дают переводы строк, `<li>` — «- », `<hN>` — `#`-заголовки,
//!   ячейки таблиц — « | »; пробелы схлопываются, кроме `<pre>`;
//! * распространённые именованные и все числовые сущности декодируются.
//!
//! Битый HTML не ошибка: незакрытые теги и одинокий `<` переживаются, худший
//! исход — лишний текст в выжимке. Поэтому выбрасываемый элемент без
//! закрывающего тега дальше по документу (`<nav>` без `</nav>`) не
//! выбрасывается: его содержимое идёт как обычная разметка, а не съедает
//! остаток страницы.

/// Результат конверсии.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HtmlText {
    /// Содержимое `<title>`, схлопнутое в одну строку; `None` — нет или пустой.
    pub(crate) title: Option<String>,
    /// Читаемый текст страницы.
    pub(crate) text: String,
}

/// Элементы, чьё содержимое — не разметка, а сырой текст до закрывающего тега.
const RAW_TEXT: [&str; 4] = ["script", "style", "textarea", "title"];

/// Элементы, чьё содержимое выбрасывается целиком (вложенность учитывается).
const SKIP_CONTENT: [&str; 9] = [
    "noscript", "template", "svg", "math", "iframe", "object", "canvas", "select", "nav",
];

/// Блочные элементы: граница абзаца до и после.
const PARAGRAPH: [&str; 22] = [
    "p",
    "div",
    "section",
    "article",
    "main",
    "header",
    "footer",
    "aside",
    "ul",
    "ol",
    "dl",
    "table",
    "blockquote",
    "pre",
    "figure",
    "form",
    "fieldset",
    "details",
    "address",
    "center",
    "body",
    "hr",
];

/// Элементы, дающие перевод строки (но не пустую строку).
const LINE: [&str; 8] = [
    "br",
    "tr",
    "dt",
    "dd",
    "figcaption",
    "summary",
    "legend",
    "caption",
];

/// Конвертировать HTML в читаемый текст.
pub(crate) fn html_to_text(html: &str) -> HtmlText {
    // Позиции в нижнем регистре совпадают побайтно: ASCII-lowercase не меняет
    // длину, а многобайтные символы не трогает.
    let lower = html.to_ascii_lowercase();
    // Последний закрывающий тег каждого из SKIP_CONTENT — один проход на имя,
    // а не поиск на каждый открывающий: иначе страница из тысяч незакрытых
    // `<svg>` разбиралась бы квадратично.
    let last_close: Vec<Option<usize>> = SKIP_CONTENT
        .iter()
        .map(|name| last_closing_tag(&lower, name))
        .collect();
    let mut out = Writer::default();
    let mut title: Option<String> = None;
    // Внутри элемента с выброшенным содержимым: имя и глубина вложенности.
    let mut skipping: Option<(String, usize)> = None;
    let mut pre_depth = 0usize;
    let mut pos = 0usize;

    while pos < html.len() {
        let Some(offset) = html[pos..].find('<') else {
            if skipping.is_none() {
                out.text(&decode_entities(&html[pos..]), pre_depth > 0);
            }
            break;
        };
        let lt = pos + offset;
        if lt > pos && skipping.is_none() {
            out.text(&decode_entities(&html[pos..lt]), pre_depth > 0);
        }

        // Комментарий, doctype, CDATA, processing instruction — пропустить.
        if lower[lt..].starts_with("<!--") {
            pos = lower[lt + 4..]
                .find("-->")
                .map_or(html.len(), |end| lt + 4 + end + 3);
            continue;
        }
        if lower[lt..].starts_with("<!") || lower[lt..].starts_with("<?") {
            pos = html[lt..].find('>').map_or(html.len(), |end| lt + end + 1);
            continue;
        }

        let Some(tag) = parse_tag(html, lt) else {
            // Одинокий `<` («a < b») — обычный текст.
            if skipping.is_none() {
                out.text("<", pre_depth > 0);
            }
            pos = lt + 1;
            continue;
        };
        pos = tag.end;
        let name = tag.name.as_str();

        if let Some((skipped, depth)) = skipping.as_mut() {
            if name == skipped {
                if tag.closing {
                    *depth -= 1;
                    if *depth == 0 {
                        skipping = None;
                    }
                } else if !tag.self_closing {
                    *depth += 1;
                }
            }
            continue;
        }

        if !tag.closing && RAW_TEXT.contains(&name) {
            // Сырой текст до `</name` — без разбора тегов внутри.
            let close = format!("</{name}");
            let content_end = lower[pos..].find(&close).map_or(html.len(), |i| pos + i);
            if name == "title" && title.is_none() {
                let text = collapse_whitespace(&decode_entities(&html[pos..content_end]));
                if !text.is_empty() {
                    title = Some(text);
                }
            }
            pos = html[content_end..]
                .find('>')
                .map_or(html.len(), |i| content_end + i + 1);
            continue;
        }

        if let Some(index) = SKIP_CONTENT.iter().position(|skip| *skip == name) {
            // Не закрыт дальше по документу — не выбрасываем: иначе он съел
            // бы весь остаток страницы.
            let closed_later = last_close[index].is_some_and(|close| close >= pos);
            if !tag.closing && !tag.self_closing && closed_later {
                skipping = Some((tag.name.clone(), 1));
            }
            continue;
        }

        if let Some(level) = heading_level(name) {
            out.newline(2);
            if !tag.closing {
                out.raw(&format!("{} ", "#".repeat(level)));
            }
            continue;
        }
        match name {
            "br" => out.line_break(),
            "li" if !tag.closing => {
                out.newline(1);
                out.raw("- ");
            }
            "td" | "th" if !tag.closing => out.cell(),
            "pre" => {
                out.newline(2);
                if tag.closing {
                    pre_depth = pre_depth.saturating_sub(1);
                } else {
                    pre_depth += 1;
                }
            }
            _ if PARAGRAPH.contains(&name) => out.newline(2),
            _ if LINE.contains(&name) => out.newline(1),
            _ => {}
        }
    }

    HtmlText {
        title,
        text: out.finish(),
    }
}

/// Начало последнего закрывающего тега `</name` в `lower` (документ в нижнем
/// регистре): `</nav>`, `</nav >` и `</nav` в самом конце, но не `</navbar>`.
fn last_closing_tag(lower: &str, name: &str) -> Option<usize> {
    let needle = format!("</{name}");
    lower
        .rmatch_indices(&needle)
        .map(|(start, _)| start)
        .find(|start| {
            !lower
                .as_bytes()
                .get(start + needle.len())
                .is_some_and(is_name_byte)
        })
}

/// Байт имени тега (после первой буквы).
fn is_name_byte(b: &u8) -> bool {
    b.is_ascii_alphanumeric() || *b == b'-' || *b == b':'
}

/// Уровень заголовка `h1`–`h6`.
fn heading_level(name: &str) -> Option<usize> {
    match name.as_bytes() {
        [b'h', level @ b'1'..=b'6'] => Some(usize::from(level - b'0')),
        _ => None,
    }
}

/// Разобранный тег.
struct Tag {
    /// Имя в нижнем регистре.
    name: String,
    /// `</name>`.
    closing: bool,
    /// `<name/>`.
    self_closing: bool,
    /// Байт сразу после `>`.
    end: usize,
}

/// Разобрать тег, начинающийся в `start` (там `<`). `None` — это не тег.
///
/// Конец тега ищется с учётом кавычек в атрибутах: `>` внутри `"…"` тег не
/// закрывает.
fn parse_tag(html: &str, start: usize) -> Option<Tag> {
    let bytes = html.as_bytes();
    let mut i = start + 1;
    let closing = bytes.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    let name_start = i;
    if !bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    while bytes.get(i).is_some_and(is_name_byte) {
        i += 1;
    }
    let name = html[name_start..i].to_ascii_lowercase();

    let mut quote: Option<u8> = None;
    while let Some(&b) = bytes.get(i) {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => {
                let self_closing = i > name_start && bytes[i - 1] == b'/';
                return Some(Tag {
                    name,
                    closing,
                    self_closing,
                    end: i + 1,
                });
            }
            None => {}
        }
        i += 1;
    }
    // Тег не закрыт до конца документа — остаток считаем тегом.
    Some(Tag {
        name,
        closing,
        self_closing: false,
        end: html.len(),
    })
}

/// Накопитель текста: схлопывает пробелы и копит переводы строк, не давая
/// больше одной пустой строки подряд.
#[derive(Default)]
struct Writer {
    buf: String,
    /// Сколько переводов строки вставить перед следующим текстом (0–2).
    pending_newlines: usize,
    /// Нужен ли пробел перед следующим текстом.
    pending_space: bool,
}

impl Writer {
    fn at_line_start(&self) -> bool {
        self.buf.is_empty() || self.buf.ends_with('\n')
    }

    /// Запросить `n` переводов строки (1 — новая строка, 2 — абзац).
    fn newline(&mut self, n: usize) {
        self.pending_newlines = self.pending_newlines.max(n);
        self.pending_space = false;
    }

    /// `<br>`: перевод строки, даже несколько подряд — но не больше абзаца.
    fn line_break(&mut self) {
        if self.pending_newlines < 2 {
            self.pending_newlines += 1;
        }
        self.pending_space = false;
    }

    /// Разделитель ячеек таблицы.
    fn cell(&mut self) {
        if self.pending_newlines == 0 && !self.at_line_start() {
            self.pending_space = false;
            self.raw(" | ");
        }
    }

    fn flush_newlines(&mut self) {
        if self.pending_newlines > 0 && !self.buf.is_empty() {
            let trimmed = self.buf.trim_end_matches([' ', '\t']).len();
            self.buf.truncate(trimmed);
            let have = self.buf.len() - self.buf.trim_end_matches('\n').len();
            for _ in have..self.pending_newlines {
                self.buf.push('\n');
            }
        }
        self.pending_newlines = 0;
    }

    /// Служебный текст (маркер списка, `#`) — без схлопывания.
    fn raw(&mut self, text: &str) {
        self.flush_newlines();
        if self.pending_space && !self.at_line_start() {
            self.buf.push(' ');
        }
        self.pending_space = false;
        self.buf.push_str(text);
    }

    /// Текст документа: вне `<pre>` пробелы схлопываются.
    fn text(&mut self, text: &str, preformatted: bool) {
        if preformatted {
            if text.is_empty() {
                return;
            }
            self.flush_newlines();
            self.buf.push_str(&text.replace("\r\n", "\n"));
            return;
        }
        for ch in text.chars() {
            if ch.is_whitespace() {
                self.pending_space = true;
                continue;
            }
            self.flush_newlines();
            if self.pending_space && !self.at_line_start() && !self.buf.ends_with(' ') {
                self.buf.push(' ');
            }
            self.pending_space = false;
            self.buf.push(ch);
        }
    }

    /// Готовый текст: хвостовые пробелы строк срезаны, подряд — не больше
    /// одной пустой строки (в том числе внутри `<pre>`: контекст дороже).
    fn finish(self) -> String {
        let mut lines: Vec<&str> = Vec::new();
        for line in self.buf.lines().map(str::trim_end) {
            if line.is_empty() && lines.last().is_some_and(|last| last.is_empty()) {
                continue;
            }
            lines.push(line);
        }
        lines.join("\n").trim().to_string()
    }
}

/// Схлопнуть пробельные последовательности в один пробел и обрезать края.
pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Декодировать HTML-сущности: числовые — все, именованные — ходовые.
/// Незнакомая сущность остаётся как есть.
pub(crate) fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // Сущность — до `;`, не длиннее 32 байт. `;` — ASCII, так что срез по
        // его позиции не режет многобайтный символ.
        let decoded = rest.as_bytes()[1..]
            .iter()
            .take(32)
            .position(|&b| b == b';')
            .and_then(|semi| decode_entity(&rest[1..1 + semi]).map(|text| (text, semi + 2)));
        match decoded {
            Some((replacement, consumed)) => {
                out.push_str(replacement.as_str());
                rest = &rest[consumed..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Одна сущность без `&` и `;`.
fn decode_entity(entity: &str) -> Option<String> {
    if let Some(number) = entity.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse::<u32>().ok()?,
        };
        let ch = char::from_u32(code)
            .filter(|ch| *ch != '\0')
            .unwrap_or('\u{fffd}');
        return Some(ch.to_string());
    }
    let text = match entity {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" | "ensp" | "emsp" | "thinsp" => " ",
        "shy" | "zwj" | "zwnj" => "",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "minus" => "−",
        "laquo" => "«",
        "raquo" => "»",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "bdquo" => "„",
        "bull" => "•",
        "middot" => "·",
        "times" => "×",
        "divide" => "÷",
        "deg" => "°",
        "euro" => "€",
        "pound" => "£",
        "larr" => "←",
        "rarr" => "→",
        "uarr" => "↑",
        "darr" => "↓",
        "le" => "≤",
        "ge" => "≥",
        "ne" => "≠",
        "para" => "¶",
        "sect" => "§",
        _ => return None,
    };
    Some(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(html: &str) -> String {
        html_to_text(html).text
    }

    #[test]
    fn extracts_title_and_body_text() {
        let page = html_to_text(
            "<!DOCTYPE html><html><head><meta charset=utf-8><title> Hello\n  &amp; World </title>\
             <link rel=stylesheet href=x.css></head><body><h1>Header</h1><p>First <b>bold</b> \
             paragraph.</p><p>Second</p></body></html>",
        );
        assert_eq!(page.title.as_deref(), Some("Hello & World"));
        assert_eq!(page.text, "# Header\n\nFirst bold paragraph.\n\nSecond");
    }

    #[test]
    fn scripts_styles_and_navigation_are_dropped() {
        let out = text(
            "<nav><a href=/>Home</a><nav>inner</nav> Menu</nav>\
             <script>var x = '<p>not text</p>';</script>\
             <style>p { color: red }</style><noscript>enable js</noscript>\
             <svg><title>icon</title><path d='M0'/></svg><p>Body</p>",
        );
        assert_eq!(out, "Body");
    }

    /// Выбрасываемый элемент без закрывающего тега не съедает остаток
    /// страницы: его содержимое идёт как обычный текст.
    #[test]
    fn unclosed_skipped_element_does_not_eat_the_rest_of_the_page() {
        assert_eq!(text("<nav>Menu<p>Body text"), "Menu\n\nBody text");
        assert_eq!(
            text("<p>Intro</p><svg><path d='M0'/><p>Rest</p>"),
            "Intro\n\nRest"
        );
        assert_eq!(
            text("<p>Intro</p><SELECT><option>a<p>Rest"),
            "Intro\n\na\n\nRest"
        );
        // `</navbar>` — не закрывающий `nav`.
        assert_eq!(text("<nav>Menu</navbar><p>Body"), "Menu\n\nBody");
        // Закрывающий тег дальше есть — выбрасывается как раньше, и `</nav`
        // без `>` в самом конце тоже закрывает.
        assert_eq!(text("<p>a</p><nav>Menu</nav ><p>b</p>"), "a\n\nb");
        assert_eq!(text("<p>a</p><nav>Menu</nav"), "a");
        // Закрывающий раньше открывающего не в счёт.
        assert_eq!(text("</nav><p>a</p><nav>Menu<p>b"), "a\n\nMenu\n\nb");
    }

    #[test]
    fn svg_title_does_not_become_the_page_title() {
        let page = html_to_text("<svg><title>icon</title></svg><title>Real</title><p>x</p>");
        assert_eq!(page.title.as_deref(), Some("Real"));
    }

    #[test]
    fn whitespace_collapses_outside_pre_and_survives_inside() {
        let out = text("<p>a\n   b\t\tc</p><pre>fn main() {\n    x  = 1;\n}</pre><p>after</p>");
        assert_eq!(out, "a b c\n\nfn main() {\n    x  = 1;\n}\n\nafter");
    }

    #[test]
    fn lists_tables_and_breaks_are_laid_out() {
        let out = text(
            "<ul><li>one</li><li>two <i>words</i></li></ul>\
             <table><tr><th>k</th><th>v</th></tr><tr><td>a</td><td>1</td></tr></table>\
             line1<br>line2<br/><br>line3",
        );
        assert_eq!(
            out,
            "- one\n- two words\n\nk | v\na | 1\n\nline1\nline2\n\nline3"
        );
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            decode_entities("&lt;a&gt; &amp;amp; &#65;&#x42;&#X43; &nbsp;&mdash; &bogus; & x"),
            "<a> &amp; ABC  — &bogus; & x"
        );
        assert_eq!(decode_entities("&#0;&#xD800;"), "\u{fffd}\u{fffd}");
        assert_eq!(text("<p>Fish &amp; chips</p>"), "Fish & chips");
        // Многобайтный текст сразу за сущностью её не ломает.
        assert_eq!(decode_entities("&amp;Привет&#33;"), "&Привет!");
    }

    #[test]
    fn broken_markup_does_not_lose_text() {
        assert_eq!(text("a < b and c > d"), "a < b and c > d");
        assert_eq!(text("<p>unclosed <b>bold"), "unclosed bold");
        assert_eq!(text("<p title=\"x > y\">quoted</p>"), "quoted");
        assert_eq!(text("<!-- <p>hidden</p> -->shown"), "shown");
        assert_eq!(text("<p>tail <a href='x"), "tail");
        assert_eq!(text(""), "");
    }

    #[test]
    fn no_title_is_none() {
        assert_eq!(html_to_text("<p>x</p>").title, None);
        assert_eq!(html_to_text("<title>  </title><p>x</p>").title, None);
    }

    #[test]
    fn uppercase_tags_are_understood() {
        let page = html_to_text("<HTML><TITLE>T</TITLE><BODY><SCRIPT>x()</SCRIPT><P>Hi</P>");
        assert_eq!(page.title.as_deref(), Some("T"));
        assert_eq!(page.text, "Hi");
    }

    #[test]
    fn multibyte_text_is_preserved() {
        assert_eq!(
            text("<p>Привет, мир — ünïcödé</p><p>日本語</p>"),
            "Привет, мир — ünïcödé\n\n日本語"
        );
    }
}
