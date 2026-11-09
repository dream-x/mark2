//! Отрисовка [`App`] через ratatui.
//!
//! Экран сверху вниз: лента, очередь ввода (если есть), поле ввода в рамке с
//! подсказкой клавиш, статус-бар. Запрос подтверждения — модалка поверх
//! всего, кроме статус-бара.
//!
//! Отрисовка почти чистая: читает состояние и пишет в [`App::view`] только
//! размеры (высоту ленты, число строк), по которым [`App`] считает прокрутку.
//! Перенос строк — [`super::wrap`], одинаковый для расчёта и для рисования.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::app::{App, Entry, NoticeKind, ToolEntry, ToolStatus};
use super::wrap::{display_text, str_width, truncate, wrap_ranges, wrap_text};

/// Кадры спиннера «ход идёт».
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Сколько последних строк промежуточного вывода тула видно в ленте.
const TOOL_OUTPUT_LINES: usize = 5;

/// Сколько элементов очереди ввода видно над полем.
const QUEUE_ROWS: usize = 3;

/// Метка сообщения пользователя.
const USER_PREFIX: &str = "› ";

/// Подсказка клавиш под полем ввода.
const HINT: &str = " Enter — отправить · Alt+Enter/Ctrl+J — перенос · PgUp/PgDn — лента · \
                    Ctrl+T — рассуждения · Ctrl+D — выход ";

/// Заголовок поля ввода во время хода.
const BUSY_TITLE: &str = " ход идёт · Enter — в очередь · Esc — прервать ";

/// Подсказка в пустом поле.
const PLACEHOLDER: &str = "Напиши задачу…";

/// Подсказка клавиш модалки подтверждения.
const APPROVAL_HINT: &str = " y — разрешить · n/Esc — запретить · Ctrl+C — прервать ход ";

/// Нарисовать кадр.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Строка ввода на колонку уже рамки: курсору в конце строки есть место.
    let input = InputLayout::new(
        app.input.text(),
        app.input.cursor(),
        (area.width as usize).saturating_sub(3),
    );
    let max_input_rows = (area.height as usize / 3).max(1);
    let input_rows = input.rows.len().clamp(1, max_input_rows);
    let queue = queue_lines(app, area.width as usize);

    let [feed_area, queue_area, input_area, status_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(queue.len() as u16),
        Constraint::Length(input_rows as u16 + 2),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_feed(frame, app, feed_area);
    frame.render_widget(Paragraph::new(queue), queue_area);
    draw_input(frame, app, &input, input_area);
    draw_status(frame, app, status_area);
    if app.approval().is_some() {
        let modal_area = Rect {
            height: area.height.saturating_sub(1),
            ..area
        };
        draw_approval(frame, app, modal_area);
    }
}

/// Вид записи — для пустых строк между записями.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    Model,
    Tool,
    Notice,
}

/// Вид записи, если она сейчас видна.
fn visible_kind(entry: &Entry, show_thinking: bool) -> Option<Kind> {
    match entry {
        Entry::User(_) => Some(Kind::User),
        Entry::Assistant(text) => (!text.trim().is_empty()).then_some(Kind::Model),
        Entry::Thinking { text, promoted } => {
            ((show_thinking || *promoted) && !text.trim().is_empty()).then_some(Kind::Model)
        }
        Entry::Tool(_) => Some(Kind::Tool),
        Entry::Notice { .. } => Some(Kind::Notice),
    }
}

/// Нужна ли пустая строка между соседними записями: вокруг сообщений
/// пользователя и перед текстом модели после тулов и служебных строк.
fn gap(previous: Kind, next: Kind) -> bool {
    next == Kind::User || previous == Kind::User || (next == Kind::Model && previous != Kind::Model)
}

/// Строки ленты при ширине `width`, уже перенесённые.
pub fn feed_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut previous = None;
    for entry in &app.entries {
        let Some(kind) = visible_kind(entry, app.show_thinking) else {
            continue;
        };
        if previous.is_some_and(|previous| gap(previous, kind)) {
            out.push(Line::default());
        }
        push_entry(&mut out, entry, app.show_thinking, width);
        previous = Some(kind);
    }
    out
}

fn push_entry(out: &mut Vec<Line<'static>>, entry: &Entry, show_thinking: bool, width: usize) {
    match entry {
        Entry::User(text) => {
            let prefix = Span::styled(USER_PREFIX, Style::new().fg(Color::Cyan).bold());
            push_wrapped(
                out,
                &display_text(text),
                width,
                Style::new().bold(),
                Some(prefix),
            );
        }
        Entry::Assistant(text) => {
            push_wrapped(
                out,
                &display_text(text.trim_matches('\n')),
                width,
                Style::new(),
                None,
            );
        }
        Entry::Thinking { text, .. } => {
            // Показанные рассуждения — приглушённо, как в CLI; продвинутые
            // в ответ при скрытых рассуждениях — обычным текстом.
            let style = if show_thinking {
                Style::new().dim().italic()
            } else {
                Style::new()
            };
            push_wrapped(out, &display_text(text.trim()), width, style, None);
        }
        Entry::Tool(tool) => push_tool(out, tool, width),
        Entry::Notice { kind, text } => {
            let style = match kind {
                NoticeKind::Info => Style::new().dim(),
                NoticeKind::Warning => Style::new().fg(Color::Yellow),
                NoticeKind::Error => Style::new().fg(Color::Red),
            };
            push_wrapped(out, &display_text(text), width, style, None);
        }
    }
}

fn push_tool(out: &mut Vec<Line<'static>>, tool: &ToolEntry, width: usize) {
    let dim = Style::new().dim();
    let header = match &tool.status {
        ToolStatus::Running => format!("{} …", tool.header),
        ToolStatus::Done => format!("{} ✓", tool.header),
        ToolStatus::Failed(_) => tool.header.clone(),
    };
    push_wrapped(out, &display_text(&header), width, dim, None);

    let output = display_text(tool.output.trim_end_matches('\n'));
    if !output.is_empty() {
        let lines: Vec<&str> = output.split('\n').collect();
        let hidden = lines.len().saturating_sub(TOOL_OUTPUT_LINES);
        if hidden > 0 {
            out.push(Line::styled(
                format!("  … ещё {hidden} {} выше", rows_word(hidden)),
                dim,
            ));
        }
        let indent = Span::styled("  ", dim);
        for line in &lines[hidden..] {
            push_wrapped(out, line, width, dim, Some(indent.clone()));
        }
    }

    if let ToolStatus::Failed(line) = &tool.status {
        push_wrapped(
            out,
            &display_text(line),
            width,
            Style::new().fg(Color::Red),
            None,
        );
    }
}

/// Перенести текст и дописать строки; `prefix` — у первой строки, у
/// остальных — отступ той же ширины.
fn push_wrapped(
    out: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    prefix: Option<Span<'static>>,
) {
    let indent = prefix.as_ref().map_or(0, |prefix| prefix.width());
    let wrap_width = width.saturating_sub(indent).max(1);
    for (index, row) in wrap_text(text, wrap_width).into_iter().enumerate() {
        let mut spans = Vec::with_capacity(2);
        if let Some(prefix) = &prefix {
            if index == 0 {
                spans.push(prefix.clone());
            } else {
                spans.push(Span::raw(" ".repeat(indent)));
            }
        }
        spans.push(Span::styled(row.to_string(), style));
        out.push(Line::from(spans));
    }
}

fn draw_feed(frame: &mut Frame, app: &mut App, area: Rect) {
    let lines = feed_lines(app, area.width as usize);
    app.view.height = area.height as usize;
    app.view.total_rows = lines.len();
    app.settle_scroll();
    let top = app.top();
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(top)
        .take(area.height as usize)
        .collect();
    frame.render_widget(Paragraph::new(visible), area);

    // Лента прокручена вверх — в правом нижнем углу, сколько осталось ниже.
    if app.scroll.is_some() && area.height > 0 {
        let below = app
            .view
            .total_rows
            .saturating_sub(top + area.height as usize);
        let label = format!(" ↓ ещё {below} {} (PgDn) ", rows_word(below));
        let width = (str_width(&label) as u16).min(area.width);
        let corner = Rect {
            x: area.right() - width,
            y: area.bottom() - 1,
            width,
            height: 1,
        };
        frame.render_widget(Clear, corner);
        frame.render_widget(
            Paragraph::new(Line::from(label).fg(Color::Yellow).reversed()),
            corner,
        );
    }
}

/// «строка» в нужном числе: 1 строка, 2 строки, 5 строк, 11 строк.
fn rows_word(count: usize) -> &'static str {
    match (count % 10, count % 100) {
        (_, 11..=14) => "строк",
        (1, _) => "строка",
        (2..=4, _) => "строки",
        _ => "строк",
    }
}

/// Очередь ввода над полем: по строке на элемент, не больше
/// [`QUEUE_ROWS`].
fn queue_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let style = Style::new().fg(Color::Yellow).dim();
    let mut lines: Vec<Line<'static>> = app
        .queue
        .iter()
        .take(QUEUE_ROWS)
        .map(|text| {
            let first = text.lines().find(|line| !line.trim().is_empty());
            let row = format!("↳ в очереди: {}", display_text(first.unwrap_or("").trim()));
            Line::styled(truncate(&row, width), style)
        })
        .collect();
    let rest = app.queue.len().saturating_sub(QUEUE_ROWS);
    if rest > 0 {
        lines.push(Line::styled(format!("  … и ещё {rest}"), style));
    }
    lines
}

/// Поле ввода, перенесённое по ширине, и место курсора в нём.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputLayout {
    /// Строки экрана.
    pub rows: Vec<String>,
    /// Строка курсора.
    pub cursor_row: usize,
    /// Колонка курсора.
    pub cursor_col: usize,
}

impl InputLayout {
    /// Разложить текст `text` с курсором на байте `cursor` по `width`
    /// колонкам.
    pub fn new(text: &str, cursor: usize, width: usize) -> Self {
        let mut rows = Vec::new();
        let mut cursor_row = 0;
        let mut cursor_col = 0;
        let mut line_start = 0;
        for line in text.split('\n') {
            let ranges = wrap_ranges(line, width);
            let last = ranges.len() - 1;
            for (index, range) in ranges.into_iter().enumerate() {
                let start = line_start + range.start;
                let end = line_start + range.end;
                if cursor >= start && (cursor < end || (index == last && cursor == end)) {
                    cursor_row = rows.len();
                    cursor_col = str_width(&text[start..cursor]);
                }
                rows.push(line[range].to_string());
            }
            line_start += line.len() + 1;
        }
        Self {
            rows,
            cursor_row,
            cursor_col,
        }
    }
}

fn draw_input(frame: &mut Frame, app: &App, input: &InputLayout, area: Rect) {
    let border = if app.busy {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().dim()
    };
    let mut block = Block::bordered()
        .border_style(border)
        .title_bottom(Line::from(HINT).dim());
    if app.busy {
        block = block.title(Line::from(BUSY_TITLE).fg(Color::Yellow));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = inner.height as usize;
    // Курсор всегда в видимой части: длинный ввод листается за ним.
    let scroll = input.cursor_row.saturating_sub(visible.saturating_sub(1));
    if app.input.is_empty() {
        frame.render_widget(Paragraph::new(Line::from(PLACEHOLDER).dim()), inner);
    } else {
        let lines: Vec<Line> = input
            .rows
            .iter()
            .skip(scroll)
            .take(visible)
            .map(|row| Line::raw(row.clone()))
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }

    if app.approval().is_none() && inner.width > 0 && inner.height > 0 {
        let x = (input.cursor_col as u16).min(inner.width - 1);
        let y = (input.cursor_row - scroll) as u16;
        frame.set_cursor_position(Position::new(inner.x + x, inner.y + y));
    }
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let spinner = SPINNER[app.tick % SPINNER.len()];
    let mut spans = vec![
        Span::styled(format!(" {}", app.model), Style::new().bold()),
        Span::raw(" · "),
        Span::styled(app.cwd.clone(), Style::new().dim()),
        Span::raw(" │ "),
    ];
    let (state, style) = if app.approval().is_some() {
        (
            "? ждёт подтверждения".to_string(),
            Style::new().fg(Color::Yellow).bold(),
        )
    } else if app.interrupting {
        (
            format!("{spinner} прерываю…"),
            Style::new().fg(Color::Yellow),
        )
    } else if app.busy {
        (
            format!("{spinner} ход идёт"),
            Style::new().fg(Color::Yellow),
        )
    } else {
        ("● готов".to_string(), Style::new().fg(Color::Green))
    };
    spans.push(Span::styled(state, style));
    if !app.queue.is_empty() {
        spans.push(Span::styled(
            format!(" · в очереди: {}", app.queue.len()),
            Style::new().fg(Color::Yellow),
        ));
    }
    if let Some(summary) = app.turn_summary() {
        spans.push(Span::raw(" │ "));
        spans.push(Span::styled(summary, Style::new().dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Модалка подтверждения: во всю ширину, по центру по высоте. Не влезает —
/// листается, а последняя строка внутри рамки говорит, какие строки вопроса
/// видны: разрешить, не дочитав, можно только по своей воле.
fn draw_approval(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(prompt) = app.approval().map(|approval| approval.prompt.clone()) else {
        return;
    };
    let inner_width = (area.width as usize).saturating_sub(2).max(1);
    let text = display_text(prompt.trim_start_matches('\n'));
    let rows: Vec<&str> = wrap_text(&text, inner_width);
    let height = (rows.len() as u16 + 2).min(area.height);
    let popup = Rect {
        y: area.y + (area.height - height) / 2,
        height,
        ..area
    };
    let inner_height = (popup.height as usize).saturating_sub(2);
    // Не влезло — одна строка внутри рамки уходит под указатель позиции.
    let overflow = rows.len() > inner_height;
    let visible = if overflow {
        inner_height.saturating_sub(1)
    } else {
        inner_height
    };
    app.view.approval_rows = rows.len();
    app.view.approval_height = visible;
    app.approval_scroll = app.approval_scroll.min(rows.len().saturating_sub(visible));

    let block = Block::bordered()
        .border_style(Style::new().fg(Color::Yellow))
        .title(Line::from(" подтверждение ").fg(Color::Yellow).bold())
        .title_bottom(Line::from(APPROVAL_HINT));
    let mut lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(app.approval_scroll)
        .take(visible)
        .map(|(index, row)| {
            let style = if index == 0 {
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if row.starts_with("allow?") {
                Style::new().bold()
            } else {
                Style::new()
            };
            Line::styled(row.to_string(), style)
        })
        .collect();
    if overflow {
        let first = app.approval_scroll + 1;
        let last = (app.approval_scroll + visible).min(rows.len());
        lines.push(
            Line::from(format!(
                "строки {first}–{last} из {} · ↑↓ PgUp/PgDn — листать",
                rows.len()
            ))
            .fg(Color::Yellow)
            .reversed(),
        );
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    use mark2_core::{Event, StopReason};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use serde_json::json;

    fn app() -> App {
        App::new("qwen3.5:9b", "~/work", false)
    }

    fn feed(app: &mut App, events: Vec<Event>) {
        let now = Instant::now();
        for event in events {
            app.on_agent_event(event, now);
        }
    }

    fn draw_into(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
    }

    /// Текст экрана построчно, без хвостовых пробелов.
    fn rows(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (area.y..area.y + area.height)
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        rows(draw_into(app, width, height).backend().buffer())
    }

    /// Стрим посреди хода: вопрос, текст, тул в работе и готовый тул,
    /// жёлтая рамка «ход идёт», спиннер в статусе.
    #[test]
    fn streaming_turn_snapshot() {
        let mut app = app();
        app.submit_text("почини тесты".into());
        feed(
            &mut app,
            vec![
                Event::TurnStarted {
                    turn_id: "t".into(),
                },
                Event::TextDelta {
                    text: "Смотрю ".into(),
                },
                Event::TextDelta {
                    text: "файлы.".into(),
                },
                Event::ToolCallStarted {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                },
                Event::ToolCallFinished {
                    id: "c1".into(),
                    content: "src".into(),
                    is_error: false,
                },
                Event::ToolCallStarted {
                    id: "c2".into(),
                    name: "read".into(),
                    input: json!({"path": "a.rs"}),
                },
            ],
        );
        assert_eq!(
            screen(&mut app, 60, 12),
            [
                "› почини тесты",
                "",
                "Смотрю файлы.",
                "→ bash {\"command\":\"ls\"} ✓",
                "→ read {\"path\":\"a.rs\"} …",
                "",
                "",
                "",
                "┌ ход идёт · Enter — в очередь · Esc — прервать ───────────┐",
                "│Напиши задачу…                                            │",
                "└ Enter — отправить · Alt+Enter/Ctrl+J — перенос · PgUp/PgD┘",
                " qwen3.5:9b · ~/work │ ⠋ ход идёт",
            ]
        );
    }

    /// Рассуждения скрыты по умолчанию, по Ctrl+T — видны и приглушены.
    #[test]
    fn thinking_toggle_snapshot() {
        let mut app = app();
        app.submit_text("вопрос".into());
        feed(
            &mut app,
            vec![
                Event::TurnStarted {
                    turn_id: "t".into(),
                },
                Event::ThinkingDelta {
                    text: "надо подумать".into(),
                },
                Event::TextDelta {
                    text: "ответ".into(),
                },
                Event::TurnFinished {
                    reason: StopReason::Completed,
                },
            ],
        );
        let hidden = screen(&mut app, 40, 8);
        assert_eq!(hidden[..4], ["› вопрос", "", "ответ", ""]);

        app.show_thinking = true;
        let terminal = draw_into(&mut app, 40, 8);
        let buffer = terminal.backend().buffer();
        assert_eq!(
            rows(buffer)[..4],
            ["› вопрос", "", "надо подумать", "ответ"]
        );
        let thinking = &buffer[(0, 2)];
        assert!(thinking.modifier.contains(Modifier::DIM), "{thinking:?}");
        assert!(thinking.modifier.contains(Modifier::ITALIC), "{thinking:?}");
        assert!(!buffer[(0, 3)].modifier.contains(Modifier::DIM));
    }

    /// Модалка подтверждения: текст `approval_prompt` целиком (многострочная
    /// команда — блоком), во всю ширину поверх ленты; курсор ввода спрятан.
    #[test]
    fn approval_modal_snapshot() {
        let mut app = app();
        app.submit_text("собери".into());
        feed(
            &mut app,
            vec![
                Event::TurnStarted {
                    turn_id: "t".into(),
                },
                Event::ApprovalRequired {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "cargo build\n./run.sh", "timeout_seconds": 60}),
                },
            ],
        );
        let terminal = draw_into(&mut app, 60, 14);
        assert_eq!(
            rows(terminal.backend().buffer()),
            [
                "› собери",
                "",
                "┌ подтверждение ───────────────────────────────────────────┐",
                "│? bash                                                    │",
                "│  command:                                                │",
                "│    cargo build                                           │",
                "│    ./run.sh                                              │",
                "│  other args: {\"timeout_seconds\":60}                      │",
                "│allow? [y/N]                                              │",
                "└ y — разрешить · n/Esc — запретить · Ctrl+C — прервать ход┘",
                "┌ ход идёт · Enter — в очередь · Esc — прервать ───────────┐",
                "│Напиши задачу…                                            │",
                "└ Enter — отправить · Alt+Enter/Ctrl+J — перенос · PgUp/PgD┘",
                " qwen3.5:9b · ~/work │ ? ждёт подтверждения",
            ]
        );
        assert!(!terminal.backend().cursor_visible());
    }

    /// Длинный запрос в маленьком окне: модалка листается, последняя строка
    /// в рамке — какие строки вопроса видны.
    #[test]
    fn long_approval_scrolls_and_says_how_much_is_hidden() {
        let mut app = app();
        app.submit_text("x".into());
        let command: Vec<String> = (1..=20).map(|n| format!("echo {n}")).collect();
        feed(
            &mut app,
            vec![
                Event::TurnStarted {
                    turn_id: "t".into(),
                },
                Event::ApprovalRequired {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": command.join("\n")}),
                },
            ],
        );
        assert_eq!(
            screen(&mut app, 60, 12),
            [
                "┌ подтверждение ───────────────────────────────────────────┐",
                "│? bash                                                    │",
                "│  command:                                                │",
                "│    echo 1                                                │",
                "│    echo 2                                                │",
                "│    echo 3                                                │",
                "│    echo 4                                                │",
                "│    echo 5                                                │",
                "│    echo 6                                                │",
                "│строки 1–8 из 23 · ↑↓ PgUp/PgDn — листать                 │",
                "└ y — разрешить · n/Esc — запретить · Ctrl+C — прервать ход┘",
                " qwen3.5:9b · ~/work │ ? ждёт подтверждения",
            ]
        );
        assert_eq!(app.view.approval_rows, 23);
        assert_eq!(app.view.approval_height, 8);

        // Листаем до конца: последняя строка вопроса видна.
        for _ in 0..30 {
            app.on_terminal_event(
                crossterm::event::Event::Key(crossterm::event::KeyEvent::from(
                    crossterm::event::KeyCode::Down,
                )),
                Instant::now(),
            );
        }
        let screen_rows = screen(&mut app, 60, 12);
        assert_eq!(
            screen_rows[7..10],
            [
                "│    echo 20                                               │",
                "│allow? [y/N]                                              │",
                "│строки 16–23 из 23 · ↑↓ PgUp/PgDn — листать               │",
            ]
        );
        assert_eq!(app.approval_scroll, 15);
    }

    /// Ввод во время хода виден над полем.
    #[test]
    fn queued_input_is_shown_above_the_field() {
        let mut app = app();
        app.submit_text("первая".into());
        app.submit_text("вторая\nс продолжением".into());
        let rows = screen(&mut app, 50, 10);
        assert_eq!(rows[5], "↳ в очереди: вторая");
        assert!(rows[9].contains("в очереди: 1"), "{rows:#?}");
    }

    /// Прокрутка: лента длиннее экрана, PgUp показывает начало, в углу ленты
    /// — сколько строк ниже.
    #[test]
    fn scrolled_feed_snapshot() {
        let mut app = app();
        for n in 1..=12 {
            app.notice(NoticeKind::Info, format!("строка {n}"));
        }
        let tail = screen(&mut app, 40, 9);
        assert_eq!(
            tail[..5],
            [
                "строка 8",
                "строка 9",
                "строка 10",
                "строка 11",
                "строка 12"
            ]
        );
        app.on_terminal_event(
            crossterm::event::Event::Key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::PageUp,
            )),
            Instant::now(),
        );
        let scrolled = screen(&mut app, 40, 9);
        assert_eq!(
            scrolled[..5],
            [
                "строка 4",
                "строка 5",
                "строка 6",
                "строка 7",
                "строка 8          ↓ ещё 4 строки (PgDn)",
            ]
        );
    }

    #[test]
    fn rows_word_agrees_with_the_number() {
        for (count, word) in [
            (1, "строка"),
            (2, "строки"),
            (4, "строки"),
            (5, "строк"),
            (11, "строк"),
            (12, "строк"),
            (21, "строка"),
            (23, "строки"),
            (100, "строк"),
        ] {
            assert_eq!(rows_word(count), word, "{count}");
        }
    }

    /// Многострочный ввод растёт вверх, курсор — в конце набранного.
    #[test]
    fn multiline_input_grows_and_places_the_cursor() {
        let mut app = app();
        app.input.insert_str("первая\nвторая");
        let mut terminal = draw_into(&mut app, 30, 12);
        let rows = rows(terminal.backend().buffer());
        assert_eq!(rows[7], "┌────────────────────────────┐");
        assert_eq!(rows[8], "│первая                      │");
        assert_eq!(rows[9], "│вторая                      │");
        terminal
            .backend_mut()
            .assert_cursor_position((1 + "вторая".chars().count() as u16, 9));
    }

    /// Крошечное окно (терминал сжали до пары строк) — кадр рисуется без
    /// паники, с модалкой, прокруткой и длинным вводом в том числе.
    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = app();
        app.submit_text("длинная задача ".repeat(20));
        app.input.insert_str(&"ввод ".repeat(40));
        feed(
            &mut app,
            vec![
                Event::TurnStarted {
                    turn_id: "t".into(),
                },
                Event::TextDelta {
                    text: "漢字 ".repeat(30),
                },
                Event::ApprovalRequired {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "echo 1\necho 2\necho 3"}),
                },
            ],
        );
        for (width, height) in [(1, 1), (2, 2), (3, 5), (8, 3), (12, 4), (20, 6)] {
            screen(&mut app, width, height);
            app.scroll = Some(0);
            screen(&mut app, width, height);
            app.approvals.clear();
            screen(&mut app, width, height);
            app.on_agent_event(
                Event::ApprovalRequired {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "echo"}),
                },
                Instant::now(),
            );
        }
    }

    #[test]
    fn input_layout_maps_the_cursor_through_wrapping() {
        let layout = InputLayout::new("один два\nтри", "один два\nт".len(), 5);
        assert_eq!(layout.rows, ["один ", "два", "три"]);
        assert_eq!((layout.cursor_row, layout.cursor_col), (2, 1));

        // На стыке перенесённых строк курсор — в начале следующей.
        let layout = InputLayout::new("один два", "один ".len(), 5);
        assert_eq!((layout.cursor_row, layout.cursor_col), (1, 0));
        // В конце текста — после последнего символа.
        let layout = InputLayout::new("один", "один".len(), 5);
        assert_eq!((layout.cursor_row, layout.cursor_col), (0, 4));
        let layout = InputLayout::new("", 0, 5);
        assert_eq!(layout.rows, [""]);
    }

    /// Управляющие символы из ответа модели не доходят до экрана сырыми.
    #[test]
    fn control_characters_in_model_text_are_escaped() {
        let mut app = app();
        feed(
            &mut app,
            vec![Event::TextDelta {
                text: "ok\u{1b}[2J\rdone\tx".into(),
            }],
        );
        let lines = feed_lines(&app, 80);
        let text: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "ok\\u{1b}[2J\\rdone    x");
    }

    /// Ошибки — красным, тул с ошибкой — строкой `✗` под заголовком.
    #[test]
    fn failed_tool_and_error_are_red() {
        let mut app = app();
        feed(
            &mut app,
            vec![
                Event::ToolCallStarted {
                    id: "c1".into(),
                    name: "edit".into(),
                    input: json!({"path": "a.rs"}),
                },
                Event::ToolCallFinished {
                    id: "c1".into(),
                    content: "old_string not found".into(),
                    is_error: true,
                },
                Event::Error {
                    message: "нет связи".into(),
                },
            ],
        );
        let lines = feed_lines(&app, 80);
        let texts: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(
            texts,
            [
                "→ edit {\"path\":\"a.rs\"}",
                "✗ old_string not found",
                "error: нет связи"
            ]
        );
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Red));
        assert_eq!(lines[2].spans[0].style.fg, Some(Color::Red));
    }
}
