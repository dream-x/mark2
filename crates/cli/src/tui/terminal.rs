//! Терминал TUI: вход в полноэкранный режим и гарантированный выход из него.
//!
//! Полноэкранный режим — это raw mode, alternate screen, захват мыши (колесо
//! листает ленту) и bracketed paste (вставка многострочного текста не
//! превращается в серию Enter). Оставить что-то из этого включённым после
//! выхода — значит сломать пользователю терминал: эхо пропадает, мышь
//! печатает мусор. Поэтому восстановление ([`restore`]) зовётся на каждом
//! пути выхода:
//!
//! * обычный выход и ошибка — `Drop` у [`TerminalGuard`];
//! * паника в любом потоке — panic hook, поставленный при входе: сначала
//!   восстановить терминал, потом отдать панику прежнему hook'у, чтобы её
//!   текст напечатался уже в нормальный экран;
//! * SIGTERM/SIGHUP/SIGINT — их ловит цикл TUI и выходит штатно
//!   ([`super::run`]).
//!
//! Восстановление идемпотентно (флаг [`ACTIVE`]): второй вызов ничего не
//! пишет, так что hook и `Drop` друг другу не мешают.

use std::io::{self, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};

/// Терминал сейчас в полноэкранном режиме TUI.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Panic hook ставится один раз за процесс.
static HOOK: Once = Once::new();

/// Полноэкранный режим, пока жив этот объект.
#[derive(Debug)]
pub struct TerminalGuard {
    _private: (),
}

impl TerminalGuard {
    /// Перевести терминал в полноэкранный режим. Не вышло — терминал
    /// возвращается как был, и наружу уходит ошибка.
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        let entered = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        );
        if let Err(error) = entered {
            restore();
            return Err(error);
        }
        Ok(Self { _private: () })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Терминал всё ещё в полноэкранном режиме: `false` после [`restore`] —
/// например, из panic hook'а, пока цикл TUI ещё жив. Рисовать тогда
/// нельзя: кадр лёг бы поверх обычного экрана.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// Вернуть терминал в обычный режим. Повторный вызов — ничего не делает.
pub fn restore() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    write_restore(&mut terminal_out());
    if let Err(error) = disable_raw_mode() {
        tracing::warn!(%error, "не удалось выключить raw mode");
    }
}

/// Последовательности выхода из полноэкранного режима — в обратном входу
/// порядке, и курсор снова видим. Ошибки записи глотаются: на пути паники
/// сообщать их некуда, а оставшиеся шаги всё равно надо попытаться сделать.
fn write_restore<W: Write>(out: &mut W) {
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    );
}

/// Куда писать последовательности восстановления. В тестах — никуда: hook
/// срабатывает и в тестовом процессе, а `cargo test` пишет в настоящий
/// терминал разработчика.
fn terminal_out() -> Box<dyn Write> {
    if cfg!(test) {
        Box::new(io::sink())
    } else {
        Box::new(io::stdout())
    }
}

/// Поставить panic hook: восстановить терминал, затем прежний hook.
fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Выход пишет все четыре «выключателя»: мышь, bracketed paste,
    /// alternate screen и видимость курсора.
    #[test]
    fn restore_sequence_undoes_everything_enter_did() {
        let mut out = Vec::new();
        write_restore(&mut out);
        let text = String::from_utf8(out).unwrap();
        for sequence in [
            "\u{1b}[?1000l", // мышь: нажатия и колесо
            "\u{1b}[?1006l", // мышь: SGR-кодировка
            "\u{1b}[?2004l", // bracketed paste
            "\u{1b}[?1049l", // alternate screen
            "\u{1b}[?25h",   // курсор виден
        ] {
            assert!(text.contains(sequence), "{sequence:?} нет в {text:?}");
        }
    }

    /// Паника в любом потоке возвращает терминал (hook), повторное
    /// восстановление — пустая операция. Один тест: флаг глобальный.
    #[test]
    fn panic_hook_restores_the_terminal_once() {
        install_panic_hook();
        ACTIVE.store(true, Ordering::SeqCst);
        assert!(is_active());

        let panicked = std::thread::spawn(|| panic!("проверка hook'а TUI")).join();
        assert!(panicked.is_err());
        assert!(!is_active(), "hook должен был восстановить терминал");

        // Второй раз — ничего не делает и не паникует.
        restore();
        assert!(!is_active());
    }
}
