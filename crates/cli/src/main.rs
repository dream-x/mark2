//! `mark2` — headless CLI поверх [`mark2_core`].
//!
//! Пока — заглушка каркаса (Task 1 плана): печатает версию.

fn main() {
    println!(
        "mark2 {} (core {}, providers {})",
        env!("CARGO_PKG_VERSION"),
        mark2_core::VERSION,
        mark2_providers::VERSION,
    );
}
