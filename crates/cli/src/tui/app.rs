//! Состояние TUI и его обновление.
//!
//! [`App`] — чистый автомат: на вход [`Event`] ядра или событие терминала
//! (клавиша, вставка, колесо), на выход — список [`Effect`] (отправить
//! [`Op`], выйти). Ни ввода-вывода, ни каналов, ни часов: текущее время
//! передаётся параметром, поэтому любой сценарий — стрим ответа, модалка
//! подтверждения, очередь ввода — прогоняется в тесте без терминала и без
//! ядра. Рисует состояние [`super::ui`], исполняет эффекты цикл в
//! [`super::run`].
//!
//! Текст строк — те же заготовки, что у CLI ([`crate::render`]): `→ tool`,
//! `✓`/`✗`, `[agent] <guard>: …`, `error: …`, запрос подтверждения из
//! [`render::approval_prompt`] (ключевое поле целиком, остальное с капом,
//! управляющие символы экранированы). Свой текст TUI не изобретает.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use mark2_core::message::Usage;
use mark2_core::{Event, Op, StopReason};
use serde_json::Value;

use super::input::InputBuffer;
use crate::print::accumulate;
use crate::render::{self, RenderOptions};

/// Сколько клавиши игнорируются после появления запроса подтверждения.
///
/// Запрос всплывает посреди набора текста (ввод во время хода разрешён), и
/// без паузы очередная буква `y` из «yes, and then…» молча разрешила бы
/// `bash`. За 400 мс человек успевает увидеть модалку, а случайный хвост
/// набора — нет.
pub const APPROVAL_GUARD: Duration = Duration::from_millis(400);

/// На сколько строк прокручивает ленту колесо мыши.
const WHEEL_ROWS: usize = 3;

/// Что сделать снаружи автомата.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Отправить команду ядру.
    Submit(Op),
    /// Выйти из TUI.
    Quit,
}

/// Запись ленты.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// Сообщение пользователя.
    User(String),
    /// Текст ответа модели (копится из `TextDelta`).
    Assistant(String),
    /// Рассуждения модели (копятся из `ThinkingDelta`).
    Thinking {
        /// Текст рассуждений.
        text: String,
        /// Модель положила в рассуждения весь ответ: они показываются и при
        /// скрытых рассуждениях — иначе ответа на экране не было бы вовсе.
        promoted: bool,
    },
    /// Тул-колл.
    Tool(ToolEntry),
    /// Служебная строка: ошибка, предупреждение, предохранитель, компакция.
    Notice {
        /// Оформление.
        kind: NoticeKind,
        /// Текст.
        text: String,
    },
}

/// Строка тул-колла в ленте.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEntry {
    /// `→ name args` — как в CLI.
    pub header: String,
    /// Промежуточный вывод (`ToolCallOutput`).
    pub output: String,
    /// Чем кончился вызов.
    pub status: ToolStatus,
}

/// Состояние тул-колла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    /// Ещё выполняется.
    Running,
    /// Успех: `✓`.
    Done,
    /// Ошибка, запрет или блокировка: `✗ <начало текста>`.
    Failed(String),
}

/// Оформление служебной строки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// Приглушённо: компакция, прерывание, решения по подтверждениям.
    Info,
    /// Жёлтым: предупреждения и предохранители.
    Warning,
    /// Красным: ошибки.
    Error,
}

/// Запрос подтверждения, ждущий ответа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    /// Идентификатор тул-колла — в ответный [`Op::Approve`].
    pub id: String,
    /// Имя тула — для строки решения в ленте.
    pub name: String,
    /// Текст вопроса — [`render::approval_prompt`] без цвета.
    pub prompt: String,
}

/// Размеры, которые знает только отрисовка: их записывает [`super::ui::draw`],
/// а прокрутка по ним считает страницу и границы.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct View {
    /// Высота ленты на экране (строк).
    pub height: usize,
    /// Сколько всего строк в ленте при текущей ширине.
    pub total_rows: usize,
    /// Высота видимой части модалки подтверждения.
    pub approval_height: usize,
    /// Сколько строк в тексте модалки.
    pub approval_rows: usize,
}

/// Всё состояние TUI.
#[derive(Debug)]
pub struct App {
    /// Модель — для статус-бара.
    pub model: String,
    /// Рабочая директория (с `~` вместо `$HOME`) — для статус-бара.
    pub cwd: String,
    /// Лента.
    pub entries: Vec<Entry>,
    /// Поле ввода.
    pub input: InputBuffer,
    /// Ввод, отправленный во время хода: уйдёт ядру после `TurnFinished`.
    pub queue: VecDeque<String>,
    /// Запросы подтверждения; модалка показывает первый.
    pub approvals: VecDeque<PendingApproval>,
    /// Прокрутка текста модалки (строк от начала).
    pub approval_scroll: usize,
    /// Идёт ли ход (с момента отправки ввода до `TurnFinished`).
    pub busy: bool,
    /// Прерывание отправлено, ядро ещё не закрыло ход.
    pub interrupting: bool,
    /// Показывать рассуждения (Ctrl+T, `--show-thinking`).
    pub show_thinking: bool,
    /// Расход токенов текущего (или последнего) хода.
    pub turn_usage: Usage,
    /// Сколько тул-коллов хода спасло приведение аргументов.
    pub turn_coerced: usize,
    /// Был ли в ходе хоть один `Usage`.
    pub turn_has_usage: bool,
    /// Расход за всю сессию — в итоговую строку после выхода.
    pub session_usage: Usage,
    /// Приведённые тул-коллы за всю сессию.
    pub session_coerced: usize,
    /// Прокрутка ленты: `None` — следовать за хвостом, `Some(n)` — верхняя
    /// видимая строка, пока пользователь читает выше.
    pub scroll: Option<usize>,
    /// Размеры с последней отрисовки.
    pub view: View,
    /// Счётчик кадров спиннера.
    pub tick: usize,
    /// Дельты дописываются в последнюю запись (сообщение модели не
    /// закончено).
    stream_open: bool,
    /// С какой записи началось текущее сообщение модели.
    message_start: usize,
    /// Тул-коллы ленты по id.
    tools: HashMap<String, usize>,
    /// Имя и аргументы тул-коллов по id (из `MessageComplete`): у
    /// запрещённого или заблокированного вызова `ToolCallStarted` не бывает.
    calls: HashMap<String, (String, Value)>,
    /// Предыдущая клавиша была Ctrl+C.
    last_ctrl_c: bool,
    /// Когда на экране появился текущий запрос подтверждения.
    approval_shown_at: Option<Instant>,
}

/// Оформление CLI без цвета: цвет в TUI задаёт ratatui.
fn plain() -> RenderOptions {
    RenderOptions::interactive(false, false)
}

impl App {
    /// Пустое состояние.
    pub fn new(model: impl Into<String>, cwd: impl Into<String>, show_thinking: bool) -> Self {
        Self {
            model: model.into(),
            cwd: cwd.into(),
            entries: Vec::new(),
            input: InputBuffer::default(),
            queue: VecDeque::new(),
            approvals: VecDeque::new(),
            approval_scroll: 0,
            busy: false,
            interrupting: false,
            show_thinking,
            turn_usage: Usage::default(),
            turn_coerced: 0,
            turn_has_usage: false,
            session_usage: Usage::default(),
            session_coerced: 0,
            scroll: None,
            view: View::default(),
            tick: 0,
            stream_open: false,
            message_start: 0,
            tools: HashMap::new(),
            calls: HashMap::new(),
            last_ctrl_c: false,
            approval_shown_at: None,
        }
    }

    /// Добавить служебную строку.
    pub fn notice(&mut self, kind: NoticeKind, text: impl Into<String>) {
        self.close_stream();
        self.entries.push(Entry::Notice {
            kind,
            text: text.into(),
        });
    }

    /// Отправить ввод: вне хода — сразу ядру, во время хода — в очередь.
    ///
    /// # Ввод посреди хода
    ///
    /// Ядро такой [`Op::UserInput`] не теряет: кладёт в свой буфер и
    /// запускает отдельным ходом после текущего. Но буфер ядра невидим и
    /// неотзываем: `Op::Interrupt` его не чистит, и Esc «остановись» тут же
    /// запустил бы отложенное. Поэтому очередь живёт здесь (steering-lite):
    /// она видна над полем ввода, после `TurnFinished` первый элемент уходит
    /// ядру новым ходом, а после прерывания очередь целиком возвращается в
    /// поле ввода — прерывание значит «стоп», и решать, отправлять ли, снова
    /// человеку.
    pub fn submit_text(&mut self, text: String) -> Vec<Effect> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        if self.busy {
            self.queue.push_back(text);
            return Vec::new();
        }
        self.start_turn(text)
    }

    fn start_turn(&mut self, text: String) -> Vec<Effect> {
        self.close_stream();
        self.entries.push(Entry::User(text.clone()));
        self.busy = true;
        self.interrupting = false;
        // Новый ход — снова следим за хвостом.
        self.scroll = None;
        vec![Effect::Submit(Op::user_input(text))]
    }

    /// Событие ядра.
    pub fn on_agent_event(&mut self, event: Event, now: Instant) -> Vec<Effect> {
        match event {
            Event::TurnStarted { .. } => {
                self.busy = true;
                self.turn_usage = Usage::default();
                self.turn_coerced = 0;
                self.turn_has_usage = false;
                self.close_stream();
                self.message_start = self.entries.len();
            }
            Event::TextDelta { text } => self.append_text(&text),
            Event::ThinkingDelta { text } => self.append_thinking(&text),
            Event::ArgumentsCoerced { .. } => {
                self.turn_coerced += 1;
                self.session_coerced += 1;
            }
            Event::ToolCallStarted { id, name, input } => {
                self.start_tool(id, &name, input);
            }
            Event::ToolCallOutput { id, chunk } => {
                if let Some(Entry::Tool(tool)) = self
                    .tools
                    .get(&id)
                    .and_then(|&index| self.entries.get_mut(index))
                {
                    tool.output.push_str(&chunk);
                }
            }
            Event::ToolCallFinished {
                id,
                content,
                is_error,
            } => self.finish_tool(id, content, is_error),
            Event::Nudge { guard, message, .. } => {
                self.notice(
                    NoticeKind::Warning,
                    render::nudge_line(guard, &message, plain()),
                );
            }
            Event::ApprovalRequired { id, name, input } => {
                if self.approvals.is_empty() {
                    self.approval_shown_at = Some(now);
                    self.approval_scroll = 0;
                }
                let prompt = render::approval_prompt(&name, &input, plain());
                self.approvals
                    .push_back(PendingApproval { id, name, prompt });
            }
            Event::MessageComplete { message } => {
                for (id, name, input) in message.tool_uses() {
                    // Локальные сервера повторяют id (`call_0`) от сообщения к
                    // сообщению: строка ленты прошлого вызова с тем же id
                    // больше не его.
                    self.tools.remove(id);
                    self.calls
                        .insert(id.to_string(), (name.to_string(), input.clone()));
                }
                if let Some(answer) = render::thinking_only_answer(&message) {
                    self.promote_thinking(answer);
                }
                self.close_stream();
                self.message_start = self.entries.len();
            }
            Event::Usage { usage } => {
                accumulate(&mut self.turn_usage, &usage);
                accumulate(&mut self.session_usage, &usage);
                self.turn_has_usage = true;
            }
            Event::Compacted {
                tokens_before,
                tokens_after,
            } => {
                self.notice(
                    NoticeKind::Info,
                    render::compaction_line(tokens_before, tokens_after, plain()),
                );
            }
            Event::TurnFinished { reason } => return self.finish_turn(reason),
            Event::Error { message } => {
                self.notice(NoticeKind::Error, render::error_line(&message, plain()));
            }
            Event::Warning { message } => {
                self.notice(NoticeKind::Warning, render::warning_line(&message, plain()));
            }
        }
        Vec::new()
    }

    fn append_text(&mut self, text: &str) {
        if self.stream_open
            && let Some(Entry::Assistant(current)) = self.entries.last_mut()
        {
            current.push_str(text);
            return;
        }
        self.entries.push(Entry::Assistant(text.to_string()));
        self.stream_open = true;
    }

    fn append_thinking(&mut self, text: &str) {
        if self.stream_open
            && let Some(Entry::Thinking { text: current, .. }) = self.entries.last_mut()
        {
            current.push_str(text);
            return;
        }
        self.entries.push(Entry::Thinking {
            text: text.to_string(),
            promoted: false,
        });
        self.stream_open = true;
    }

    fn close_stream(&mut self) {
        self.stream_open = false;
    }

    /// Ответ целиком в рассуждениях: показать их как ответ (с пометкой, как
    /// CLI). Если рассуждения не стримились дельтами — добавить их из
    /// сообщения.
    fn promote_thinking(&mut self, answer: String) {
        let start = self.message_start.min(self.entries.len());
        let mut found = false;
        for entry in &mut self.entries[start..] {
            if let Entry::Thinking { promoted, .. } = entry {
                *promoted = true;
                found = true;
            }
        }
        if !found {
            self.entries.push(Entry::Thinking {
                text: answer,
                promoted: true,
            });
        }
        self.notice(
            NoticeKind::Warning,
            render::warning_line(render::THINKING_ONLY_NOTE, plain()),
        );
    }

    fn start_tool(&mut self, id: String, name: &str, input: Value) -> usize {
        self.close_stream();
        let started = Event::ToolCallStarted {
            id: id.clone(),
            name: name.to_string(),
            input,
        };
        let header = render::render(&started, plain())
            .map(|line| line.trim().to_string())
            .unwrap_or_default();
        self.entries.push(Entry::Tool(ToolEntry {
            header,
            output: String::new(),
            status: ToolStatus::Running,
        }));
        let index = self.entries.len() - 1;
        self.tools.insert(id, index);
        index
    }

    fn finish_tool(&mut self, id: String, content: String, is_error: bool) {
        let index = match self.tools.get(&id) {
            Some(&index) => index,
            None => {
                // Запрет политики, отказ пользователя, блокировка детектором
                // зацикливания: `ToolCallStarted` не было.
                let (name, input) = self
                    .calls
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| ("?".to_string(), Value::Null));
                self.start_tool(id.clone(), &name, input)
            }
        };
        let status = if is_error {
            let finished = Event::ToolCallFinished {
                id,
                content,
                is_error,
            };
            let line = render::render(&finished, plain()).unwrap_or_default();
            ToolStatus::Failed(line.trim_end().to_string())
        } else {
            ToolStatus::Done
        };
        if let Some(Entry::Tool(tool)) = self.entries.get_mut(index) {
            tool.status = status;
        }
    }

    fn finish_turn(&mut self, reason: StopReason) -> Vec<Effect> {
        self.busy = false;
        self.interrupting = false;
        self.approvals.clear();
        self.approval_shown_at = None;
        self.close_stream();
        match reason {
            StopReason::Completed => {}
            StopReason::Cancelled => self.notice(NoticeKind::Info, "ход прерван"),
            StopReason::MaxTurns => self.notice(
                NoticeKind::Warning,
                "ход остановлен: исчерпан max_turns (задача не доделана)",
            ),
            StopReason::Error => self.notice(NoticeKind::Error, "ход завершён с ошибкой"),
        }
        if reason == StopReason::Cancelled {
            self.return_queue_to_input();
            return Vec::new();
        }
        match self.queue.pop_front() {
            Some(next) => self.start_turn(next),
            None => Vec::new(),
        }
    }

    /// После прерывания очередь не уходит сама: она возвращается в поле
    /// ввода (перед тем, что там уже набрано).
    fn return_queue_to_input(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let mut parts: Vec<String> = self.queue.drain(..).collect();
        if !self.input.is_empty() {
            parts.push(self.input.take());
        }
        self.input.set(parts.join("\n"));
    }

    /// Событие терминала.
    pub fn on_terminal_event(&mut self, event: TermEvent, now: Instant) -> Vec<Effect> {
        match event {
            TermEvent::Key(key) => self.on_key(key, now),
            TermEvent::Paste(text) => {
                // В модалку вставка не попадает: `y` из буфера обмена не
                // должен ничего разрешать.
                if self.approvals.is_empty() {
                    self.input.insert_str(&text);
                }
                Vec::new()
            }
            TermEvent::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.scroll_up(WHEEL_ROWS),
                    MouseEventKind::ScrollDown => self.scroll_down(WHEEL_ROWS),
                    _ => {}
                }
                Vec::new()
            }
            TermEvent::Resize(..) | TermEvent::FocusGained | TermEvent::FocusLost => Vec::new(),
        }
    }

    /// Кадр спиннера.
    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    fn on_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl_c = ctrl && matches!(key.code, KeyCode::Char('c' | 'C'));
        let repeated = ctrl_c && self.last_ctrl_c;
        self.last_ctrl_c = ctrl_c;
        if repeated {
            return vec![Effect::Quit];
        }
        if !self.approvals.is_empty() {
            return self.on_approval_key(key, ctrl, now);
        }

        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => return self.on_ctrl_c(),
            KeyCode::Char('d' | 'D') if ctrl => {
                if self.input.is_empty() {
                    return vec![Effect::Quit];
                }
                self.input.delete();
            }
            KeyCode::Char('t' | 'T') if ctrl => self.show_thinking = !self.show_thinking,
            KeyCode::Char('j' | 'J') if ctrl => self.input.insert_char('\n'),
            KeyCode::Char('a' | 'A') if ctrl => self.input.home(),
            KeyCode::Char('e' | 'E') if ctrl => self.input.end(),
            KeyCode::Char('u' | 'U') if ctrl => self.input.kill_to_line_start(),
            KeyCode::Char(c) if !ctrl && !alt => self.input.insert_char(c),
            KeyCode::Esc => return self.interrupt(),
            // Shift+Enter различим только в терминалах с kitty-протоколом
            // клавиатуры; Alt+Enter и Ctrl+J работают везде.
            KeyCode::Enter if alt || key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.input.insert_char('\n');
            }
            KeyCode::Enter => {
                if self.input.text().trim().is_empty() {
                    return Vec::new();
                }
                let text = self.input.take();
                return self.submit_text(text);
            }
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            // Вверх/вниз двигают курсор по строкам ввода; с крайней строки —
            // листают ленту.
            KeyCode::Up => {
                if !self.input.up() {
                    self.scroll_up(1);
                }
            }
            KeyCode::Down => {
                if !self.input.down() {
                    self.scroll_down(1);
                }
            }
            KeyCode::PageUp => self.scroll_up(self.page()),
            KeyCode::PageDown => self.scroll_down(self.page()),
            _ => {}
        }
        Vec::new()
    }

    /// Ctrl+C: во время хода — прервать; вне хода — стереть набранное, а на
    /// пустом поле — выйти. Второй Ctrl+C подряд выходит всегда (см.
    /// [`Self::on_key`]).
    fn on_ctrl_c(&mut self) -> Vec<Effect> {
        if self.busy {
            return self.interrupt();
        }
        if self.input.is_empty() {
            return vec![Effect::Quit];
        }
        self.input.take();
        Vec::new()
    }

    /// Прервать идущий ход ([`Op::Interrupt`]); вне хода — ничего.
    fn interrupt(&mut self) -> Vec<Effect> {
        if !self.busy {
            return Vec::new();
        }
        if !self.interrupting {
            self.interrupting = true;
            self.notice(NoticeKind::Info, "прерываю ход… (Ctrl+C ещё раз — выход)");
        }
        vec![Effect::Submit(Op::Interrupt)]
    }

    fn on_approval_key(&mut self, key: KeyEvent, ctrl: bool, now: Instant) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => {
                // Ядро бросит ожидание аппрува само, получив Interrupt.
                self.approvals.clear();
                self.approval_shown_at = None;
                return self.interrupt();
            }
            KeyCode::Up => self.approval_scroll = self.approval_scroll.saturating_sub(1),
            KeyCode::Down => self.scroll_approval_down(1),
            KeyCode::PageUp => {
                let page = self.view.approval_height.max(1);
                self.approval_scroll = self.approval_scroll.saturating_sub(page);
            }
            KeyCode::PageDown => self.scroll_approval_down(self.view.approval_height.max(1)),
            _ if self.approval_guarded(now) => {}
            KeyCode::Char('y' | 'Y' | 'д' | 'Д') if !ctrl => return self.decide(true, now),
            KeyCode::Char('n' | 'N' | 'т' | 'Т') if !ctrl => return self.decide(false, now),
            KeyCode::Esc => return self.decide(false, now),
            _ => {}
        }
        Vec::new()
    }

    /// Запрос только что появился — ответные клавиши ещё не принимаются.
    fn approval_guarded(&self, now: Instant) -> bool {
        self.approval_shown_at
            .is_some_and(|shown| now.saturating_duration_since(shown) < APPROVAL_GUARD)
    }

    fn scroll_approval_down(&mut self, rows: usize) {
        let max = self
            .view
            .approval_rows
            .saturating_sub(self.view.approval_height);
        self.approval_scroll = (self.approval_scroll + rows).min(max);
    }

    fn decide(&mut self, allow: bool, now: Instant) -> Vec<Effect> {
        let Some(approval) = self.approvals.pop_front() else {
            return Vec::new();
        };
        self.approval_scroll = 0;
        // Следующий запрос — снова с паузой: двойное нажатие не должно
        // разрешить два вызова подряд.
        self.approval_shown_at = (!self.approvals.is_empty()).then_some(now);
        let verdict = if allow {
            "разрешено"
        } else {
            "запрещено"
        };
        self.notice(NoticeKind::Info, format!("? {} — {verdict}", approval.name));
        let op = if allow {
            Op::allow(approval.id)
        } else {
            Op::deny(approval.id)
        };
        vec![Effect::Submit(op)]
    }

    /// Страница прокрутки: высота ленты без одной строки (для контекста).
    fn page(&self) -> usize {
        self.view.height.saturating_sub(1).max(1)
    }

    /// Самая нижняя возможная верхняя строка.
    fn max_top(&self) -> usize {
        self.view.total_rows.saturating_sub(self.view.height)
    }

    /// Верхняя видимая строка ленты.
    pub fn top(&self) -> usize {
        self.scroll.unwrap_or(usize::MAX).min(self.max_top())
    }

    fn scroll_up(&mut self, rows: usize) {
        if self.max_top() == 0 {
            return;
        }
        self.scroll = Some(self.top().saturating_sub(rows));
    }

    fn scroll_down(&mut self, rows: usize) {
        let Some(top) = self.scroll else { return };
        let top = top.saturating_add(rows);
        // Докрутили до хвоста — снова следим за ним.
        self.scroll = (top < self.max_top()).then_some(top);
    }

    /// Прокрутка упёрлась в хвост (лента дорисовалась до конца) — вернуть
    /// автоследование. Зовётся отрисовкой после пересчёта размеров.
    pub fn settle_scroll(&mut self) {
        if self.scroll.is_some_and(|top| top >= self.max_top()) {
            self.scroll = None;
        }
    }

    /// Запрос подтверждения на экране.
    pub fn approval(&self) -> Option<&PendingApproval> {
        self.approvals.front()
    }

    /// Ключевые поля итога хода для статус-бара: `tokens: in … / out … ·
    /// coerced N` — та же строка, что CLI печатает в stderr.
    pub fn turn_summary(&self) -> Option<String> {
        self.turn_has_usage
            .then(|| render::usage_summary(&self.turn_usage, self.turn_coerced, plain()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::{KeyEventState, MouseEvent};
    use mark2_core::Guard;
    use mark2_core::message::{ContentBlock, Message, Role};
    use serde_json::json;

    fn app() -> App {
        App::new("qwen", "~/work", false)
    }

    fn key(code: KeyCode) -> TermEvent {
        TermEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn alt_enter() -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT))
    }

    /// Момент, когда все паузы давно истекли.
    fn later() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            assert!(
                app.on_terminal_event(key(KeyCode::Char(c)), later())
                    .is_empty()
            );
        }
    }

    fn feed(app: &mut App, events: Vec<Event>) -> Vec<Effect> {
        let now = Instant::now();
        events
            .into_iter()
            .flat_map(|event| app.on_agent_event(event, now))
            .collect()
    }

    fn submit(text: &str) -> Effect {
        Effect::Submit(Op::user_input(text))
    }

    fn finished(reason: StopReason) -> Event {
        Event::TurnFinished { reason }
    }

    fn started() -> Event {
        Event::TurnStarted {
            turn_id: "t".into(),
        }
    }

    fn delta(text: &str) -> Event {
        Event::TextDelta { text: text.into() }
    }

    #[test]
    fn enter_submits_user_input_and_marks_the_turn_busy() {
        let mut app = app();
        type_text(&mut app, "почини тесты");
        let effects = app.on_terminal_event(key(KeyCode::Enter), later());
        assert_eq!(effects, [submit("почини тесты")]);
        assert!(app.busy);
        assert!(app.input.is_empty());
        assert_eq!(app.entries, [Entry::User("почини тесты".into())]);

        // Пустой и пробельный ввод не уходит.
        let mut idle = self::app();
        type_text(&mut idle, "  ");
        assert!(
            idle.on_terminal_event(key(KeyCode::Enter), later())
                .is_empty()
        );
        assert_eq!(idle.input.text(), "  ");
    }

    #[test]
    fn alt_enter_and_ctrl_j_insert_newlines() {
        let mut app = app();
        type_text(&mut app, "a");
        app.on_terminal_event(alt_enter(), later());
        type_text(&mut app, "b");
        app.on_terminal_event(ctrl('j'), later());
        type_text(&mut app, "c");
        assert_eq!(app.input.text(), "a\nb\nc");
        assert_eq!(
            app.on_terminal_event(key(KeyCode::Enter), later()),
            [submit("a\nb\nc")]
        );
    }

    /// Дельты одного сообщения склеиваются в одну запись; следующее
    /// сообщение хода — новая запись.
    #[test]
    fn text_deltas_stream_into_one_entry_per_message() {
        let mut app = app();
        app.submit_text("q".into());
        feed(
            &mut app,
            vec![
                started(),
                delta("Смотрю "),
                delta("файлы."),
                Event::MessageComplete {
                    message: Message::new(
                        Role::Assistant,
                        vec![
                            ContentBlock::text("Смотрю файлы."),
                            ContentBlock::tool_use("c1", "bash", json!({"command": "ls"})),
                        ],
                    ),
                },
                Event::ToolCallStarted {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                },
                Event::ToolCallFinished {
                    id: "c1".into(),
                    content: "src\n".into(),
                    is_error: false,
                },
                delta("Готово"),
                delta("."),
            ],
        );
        assert_eq!(
            app.entries,
            [
                Entry::User("q".into()),
                Entry::Assistant("Смотрю файлы.".into()),
                Entry::Tool(ToolEntry {
                    header: "→ bash {\"command\":\"ls\"}".into(),
                    output: String::new(),
                    status: ToolStatus::Done,
                }),
                Entry::Assistant("Готово.".into()),
            ]
        );
    }

    /// Вызов без `ToolCallStarted` (запрет, блокировка) всё равно попадает в
    /// ленту — с именем и аргументами из `MessageComplete` и `✗` как в CLI.
    #[test]
    fn denied_tool_without_start_is_shown_as_failed() {
        let mut app = app();
        app.submit_text("q".into());
        feed(
            &mut app,
            vec![
                started(),
                Event::MessageComplete {
                    message: Message::new(
                        Role::Assistant,
                        vec![ContentBlock::tool_use(
                            "c1",
                            "write",
                            json!({"path": "a.txt"}),
                        )],
                    ),
                },
                Event::ToolCallFinished {
                    id: "c1".into(),
                    content: "denied by user\nask again".into(),
                    is_error: true,
                },
            ],
        );
        assert_eq!(
            app.entries.last(),
            Some(&Entry::Tool(ToolEntry {
                header: "→ write {\"path\":\"a.txt\"}".into(),
                output: String::new(),
                status: ToolStatus::Failed("✗ denied by user ask again".into()),
            }))
        );
    }

    /// Сервер повторяет id от сообщения к сообщению (`call_0`): запрещённый
    /// вызов второго сообщения — новая строка, прошлая остаётся `✓`.
    #[test]
    fn reused_tool_call_ids_do_not_touch_older_entries() {
        let mut app = app();
        let message = |path: &str| Event::MessageComplete {
            message: Message::new(
                Role::Assistant,
                vec![ContentBlock::tool_use(
                    "call_0",
                    "write",
                    json!({"path": path}),
                )],
            ),
        };
        feed(
            &mut app,
            vec![
                started(),
                message("a.txt"),
                Event::ToolCallStarted {
                    id: "call_0".into(),
                    name: "write".into(),
                    input: json!({"path": "a.txt"}),
                },
                Event::ToolCallFinished {
                    id: "call_0".into(),
                    content: "ok".into(),
                    is_error: false,
                },
                message("b.txt"),
                Event::ToolCallFinished {
                    id: "call_0".into(),
                    content: "denied by user".into(),
                    is_error: true,
                },
            ],
        );
        let tools: Vec<&ToolEntry> = app
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 2, "{:?}", app.entries);
        assert_eq!(tools[0].header, "→ write {\"path\":\"a.txt\"}");
        assert_eq!(tools[0].status, ToolStatus::Done);
        assert_eq!(tools[1].header, "→ write {\"path\":\"b.txt\"}");
        assert_eq!(
            tools[1].status,
            ToolStatus::Failed("✗ denied by user".into())
        );
    }

    #[test]
    fn ctrl_t_toggles_thinking_and_flag_sets_the_default() {
        let mut app = app();
        assert!(!app.show_thinking);
        app.on_terminal_event(ctrl('t'), later());
        assert!(app.show_thinking);
        app.on_terminal_event(ctrl('t'), later());
        assert!(!app.show_thinking);
        assert!(App::new("m", "/", true).show_thinking);

        feed(
            &mut app,
            vec![
                Event::ThinkingDelta {
                    text: "хм, ".into(),
                },
                Event::ThinkingDelta {
                    text: "надо ls".into(),
                },
            ],
        );
        assert_eq!(
            app.entries,
            [Entry::Thinking {
                text: "хм, надо ls".into(),
                promoted: false
            }]
        );
    }

    /// Ответ целиком в рассуждениях — они продвигаются в ответ, плюс
    /// пометка, как в CLI.
    #[test]
    fn thinking_only_answer_is_promoted_with_a_note() {
        let mut app = app();
        feed(
            &mut app,
            vec![
                started(),
                Event::ThinkingDelta {
                    text: "Done: 17".into(),
                },
                Event::MessageComplete {
                    message: Message::new(
                        Role::Assistant,
                        vec![ContentBlock::thinking("Done: 17")],
                    ),
                },
            ],
        );
        assert_eq!(
            app.entries[0],
            Entry::Thinking {
                text: "Done: 17".into(),
                promoted: true
            }
        );
        assert_eq!(
            app.entries[1],
            Entry::Notice {
                kind: NoticeKind::Warning,
                text: format!("warning: {}", render::THINKING_ONLY_NOTE),
            }
        );

        // Рассуждения пришли только в сообщении — всё равно на экране.
        let mut quiet = self::app();
        feed(
            &mut quiet,
            vec![
                started(),
                Event::MessageComplete {
                    message: Message::new(Role::Assistant, vec![ContentBlock::thinking("ответ")]),
                },
            ],
        );
        assert_eq!(
            quiet.entries[0],
            Entry::Thinking {
                text: "ответ".into(),
                promoted: true
            }
        );
    }

    #[test]
    fn service_events_become_notices_with_cli_markers() {
        let mut app = app();
        feed(
            &mut app,
            vec![
                Event::Warning {
                    message: "память обрезана".into(),
                },
                Event::Nudge {
                    id: "c1".into(),
                    guard: Guard::WriteChurn,
                    message: "3-я запись подряд".into(),
                },
                Event::Compacted {
                    tokens_before: 12_000,
                    tokens_after: 3_000,
                },
                Event::Error {
                    message: "нет связи".into(),
                },
            ],
        );
        let texts: Vec<(NoticeKind, &str)> = app
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Notice { kind, text } => (*kind, text.as_str()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            [
                (NoticeKind::Warning, "warning: память обрезана"),
                (
                    NoticeKind::Warning,
                    "[agent] write_churn: 3-я запись подряд"
                ),
                (
                    NoticeKind::Info,
                    "[agent] compaction: контекст сжат ~12000 → ~3000 токенов"
                ),
                (NoticeKind::Error, "error: нет связи"),
            ]
        );
    }

    fn approval_event(id: &str) -> Event {
        Event::ApprovalRequired {
            id: id.into(),
            name: "bash".into(),
            input: json!({"command": "rm -rf build"}),
        }
    }

    /// `y` — allow, `n` и Esc — deny, и ровно для того id, что спрашивали;
    /// текст вопроса — `approval_prompt` CLI.
    #[test]
    fn approval_modal_answers_with_the_right_op() {
        let mut app = app();
        app.submit_text("q".into());
        feed(&mut app, vec![started(), approval_event("c1")]);
        let approval = app.approval().expect("модалка");
        assert_eq!(
            approval.prompt,
            render::approval_prompt("bash", &json!({"command": "rm -rf build"}), plain())
        );
        assert!(approval.prompt.contains("  command: rm -rf build\n"));

        assert_eq!(
            app.on_terminal_event(key(KeyCode::Char('y')), later()),
            [Effect::Submit(Op::allow("c1"))]
        );
        assert!(app.approval().is_none());

        for deny in [KeyCode::Char('n'), KeyCode::Esc, KeyCode::Char('N')] {
            app.on_agent_event(approval_event("c2"), Instant::now());
            assert_eq!(
                app.on_terminal_event(key(deny), later()),
                [Effect::Submit(Op::deny("c2"))],
                "{deny:?}"
            );
        }
        // Esc в модалке — отказ, а не прерывание хода.
        assert!(!app.interrupting);
        assert!(app.entries.iter().any(|entry| matches!(
            entry,
            Entry::Notice { text, .. } if text == "? bash — разрешено"
        )));
    }

    /// Пока модалка открыта, прочие клавиши в поле ввода не попадают, а
    /// вставка из буфера обмена ничего не разрешает.
    #[test]
    fn approval_modal_swallows_other_keys_and_paste() {
        let mut app = app();
        app.submit_text("q".into());
        feed(&mut app, vec![started(), approval_event("c1")]);
        assert!(
            app.on_terminal_event(key(KeyCode::Char('x')), later())
                .is_empty()
        );
        assert!(
            app.on_terminal_event(key(KeyCode::Enter), later())
                .is_empty()
        );
        assert!(
            app.on_terminal_event(TermEvent::Paste("y".into()), later())
                .is_empty()
        );
        assert!(app.input.is_empty());
        assert!(app.approval().is_some());
    }

    /// Первые 400 мс после появления вопроса `y` не принимается: хвост
    /// набора не разрешает тул.
    #[test]
    fn approval_keys_are_ignored_right_after_the_modal_appears() {
        let mut app = app();
        app.submit_text("q".into());
        let shown = Instant::now();
        app.on_agent_event(started(), shown);
        app.on_agent_event(approval_event("c1"), shown);
        let soon = shown + Duration::from_millis(100);
        assert!(
            app.on_terminal_event(key(KeyCode::Char('y')), soon)
                .is_empty()
        );
        assert!(app.approval().is_some());
        let after = shown + APPROVAL_GUARD;
        assert_eq!(
            app.on_terminal_event(key(KeyCode::Char('y')), after),
            [Effect::Submit(Op::allow("c1"))]
        );
    }

    /// Ctrl+C в модалке прерывает ход (ядро само бросит ожидание аппрува).
    #[test]
    fn ctrl_c_in_the_modal_interrupts_the_turn() {
        let mut app = app();
        app.submit_text("q".into());
        feed(&mut app, vec![started(), approval_event("c1")]);
        assert_eq!(
            app.on_terminal_event(ctrl('c'), later()),
            [Effect::Submit(Op::Interrupt)]
        );
        assert!(app.approval().is_none());
        assert!(app.interrupting);
    }

    /// Ввод посреди хода не уходит ядру сразу, а ждёт `TurnFinished` и
    /// отправляется отдельным ходом; очередь — по порядку.
    #[test]
    fn input_during_a_turn_is_queued_until_turn_finished() {
        let mut app = app();
        assert_eq!(app.submit_text("первая".into()), [submit("первая")]);
        feed(&mut app, vec![started(), delta("работаю")]);

        type_text(&mut app, "вторая");
        assert!(
            app.on_terminal_event(key(KeyCode::Enter), later())
                .is_empty()
        );
        type_text(&mut app, "третья");
        assert!(
            app.on_terminal_event(key(KeyCode::Enter), later())
                .is_empty()
        );
        assert_eq!(app.queue, ["вторая", "третья"]);
        assert!(app.input.is_empty());

        assert_eq!(
            feed(&mut app, vec![finished(StopReason::Completed)]),
            [submit("вторая")]
        );
        assert!(app.busy);
        assert_eq!(app.queue, ["третья"]);
        assert_eq!(app.entries.last(), Some(&Entry::User("вторая".into())));

        assert_eq!(
            feed(
                &mut app,
                vec![started(), delta("ок"), finished(StopReason::Completed)]
            ),
            [submit("третья")]
        );
        assert!(app.queue.is_empty());
    }

    /// Прерывание — «стоп»: очередь возвращается в поле ввода, ничего не
    /// уходит само.
    #[test]
    fn interrupt_returns_the_queue_to_the_input() {
        let mut app = app();
        app.submit_text("долгая".into());
        feed(&mut app, vec![started()]);
        app.submit_text("уточнение".into());
        type_text(&mut app, "черновик");

        assert_eq!(
            app.on_terminal_event(key(KeyCode::Esc), later()),
            [Effect::Submit(Op::Interrupt)]
        );
        assert!(app.interrupting);
        assert!(feed(&mut app, vec![finished(StopReason::Cancelled)]).is_empty());
        assert!(!app.busy);
        assert!(app.queue.is_empty());
        assert_eq!(app.input.text(), "уточнение\nчерновик");
        assert!(app.entries.iter().any(|entry| matches!(
            entry,
            Entry::Notice { text, .. } if text == "ход прерван"
        )));
    }

    /// Второй ход после `TurnFinished`: поле снова отправляет сразу.
    #[test]
    fn second_turn_after_turn_finished_submits_immediately() {
        let mut app = app();
        app.submit_text("один".into());
        feed(
            &mut app,
            vec![
                started(),
                delta("ответ 1"),
                Event::Usage {
                    usage: Usage::new(100, 20),
                },
                finished(StopReason::Completed),
            ],
        );
        assert!(!app.busy);
        assert_eq!(
            app.turn_summary().as_deref(),
            Some("tokens: in 100 / out 20 · coerced 0")
        );
        type_text(&mut app, "два");
        assert_eq!(
            app.on_terminal_event(key(KeyCode::Enter), later()),
            [submit("два")]
        );
        feed(
            &mut app,
            vec![
                started(),
                Event::Usage {
                    usage: Usage::new(300, 5),
                },
                finished(StopReason::Completed),
            ],
        );
        assert_eq!(
            app.turn_summary().as_deref(),
            Some("tokens: in 300 / out 5 · coerced 0")
        );
        assert_eq!(app.session_usage, Usage::new(400, 25));
    }

    /// Esc/Ctrl+C во время хода — Interrupt; Ctrl+C дважды — выход; вне хода
    /// Ctrl+C на пустом поле и Ctrl+D на пустом поле — выход.
    #[test]
    fn interrupt_and_exit_keys() {
        let mut app = app();
        app.submit_text("q".into());
        assert_eq!(
            app.on_terminal_event(ctrl('c'), later()),
            [Effect::Submit(Op::Interrupt)]
        );
        assert_eq!(app.on_terminal_event(ctrl('c'), later()), [Effect::Quit]);

        // Вне хода: Ctrl+C стирает набранное, на пустом поле — выход.
        let mut idle = self::app();
        type_text(&mut idle, "черновик");
        assert!(idle.on_terminal_event(ctrl('c'), later()).is_empty());
        assert!(idle.input.is_empty());
        type_text(&mut idle, "x");
        idle.on_terminal_event(key(KeyCode::Backspace), later());
        assert_eq!(idle.on_terminal_event(ctrl('c'), later()), [Effect::Quit]);

        // Ctrl+D — выход только на пустом поле.
        let mut idle = self::app();
        type_text(&mut idle, "ab");
        idle.on_terminal_event(key(KeyCode::Home), later());
        assert!(idle.on_terminal_event(ctrl('d'), later()).is_empty());
        assert_eq!(idle.input.text(), "b");
        idle.on_terminal_event(ctrl('d'), later());
        assert_eq!(idle.on_terminal_event(ctrl('d'), later()), [Effect::Quit]);

        // Esc вне хода — ничего.
        assert!(
            self::app()
                .on_terminal_event(key(KeyCode::Esc), later())
                .is_empty()
        );
    }

    /// Прокрутка: вверх — фиксирует верхнюю строку, вниз до хвоста —
    /// возвращает автоследование; колесо — по три строки.
    #[test]
    fn scrolling_pins_and_unpins_the_view() {
        let mut app = app();
        app.view = View {
            height: 10,
            total_rows: 50,
            ..View::default()
        };
        assert_eq!(app.top(), 40);
        app.on_terminal_event(key(KeyCode::PageUp), later());
        assert_eq!(app.scroll, Some(31));
        let wheel = |kind| {
            TermEvent::Mouse(MouseEvent {
                kind,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })
        };
        app.on_terminal_event(wheel(MouseEventKind::ScrollUp), later());
        assert_eq!(app.top(), 28);
        // Новые строки снизу не сдвигают зафиксированный вид.
        app.view.total_rows = 80;
        assert_eq!(app.top(), 28);
        app.on_terminal_event(wheel(MouseEventKind::ScrollDown), later());
        assert_eq!(app.top(), 31);
        for _ in 0..4 {
            app.on_terminal_event(key(KeyCode::PageDown), later());
        }
        assert_eq!(app.scroll, Some(67));
        app.on_terminal_event(key(KeyCode::PageDown), later());
        assert_eq!(app.scroll, None, "у хвоста — снова автоследование");
        assert_eq!(app.top(), 70);

        // Отправка нового ввода возвращает к хвосту.
        app.on_terminal_event(key(KeyCode::PageUp), later());
        assert!(app.scroll.is_some());
        app.submit_text("ещё".into());
        assert_eq!(app.scroll, None);
    }

    #[test]
    fn up_and_down_scroll_the_feed_from_the_edge_lines_of_the_input() {
        let mut app = app();
        app.view = View {
            height: 5,
            total_rows: 20,
            ..View::default()
        };
        type_text(&mut app, "a");
        app.on_terminal_event(alt_enter(), later());
        type_text(&mut app, "b");
        app.on_terminal_event(key(KeyCode::Up), later());
        assert_eq!(app.scroll, None, "первая клавиша — по строкам ввода");
        app.on_terminal_event(key(KeyCode::Up), later());
        assert_eq!(app.scroll, Some(14));
        app.on_terminal_event(key(KeyCode::Down), later());
        app.on_terminal_event(key(KeyCode::Down), later());
        assert_eq!(app.scroll, None);
    }

    #[test]
    fn key_release_events_are_ignored() {
        let mut app = app();
        let release = TermEvent::Key(KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        app.on_terminal_event(release, later());
        assert!(app.input.is_empty());
    }

    #[test]
    fn paste_goes_into_the_input_without_submitting() {
        let mut app = app();
        assert!(
            app.on_terminal_event(TermEvent::Paste("строка 1\r\nстрока 2".into()), later())
                .is_empty()
        );
        assert_eq!(app.input.text(), "строка 1\nстрока 2");
        assert!(!app.busy);
    }
}
