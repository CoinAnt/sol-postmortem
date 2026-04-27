// Conditional terminal styling. All styled output flows through these helpers,
// so a single global flag controls whether any ANSI escape codes are emitted.
//
// Why not owo-colors directly: its plain `.cyan()` etc. always emit ANSI,
// regardless of any global override. We need a single switch so --color=never
// (and Windows terminals that don't render ANSI) produce clean plain text.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(b: bool) {
    ENABLED.store(b, Ordering::Relaxed);
}

fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn wrap(code: &str, s: &str) -> String {
    if enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn cyan(s: &str) -> String {
    wrap("36", s)
}

pub fn red(s: &str) -> String {
    wrap("31", s)
}

pub fn green(s: &str) -> String {
    wrap("32", s)
}

pub fn yellow(s: &str) -> String {
    wrap("33", s)
}

pub fn dim(s: &str) -> String {
    wrap("2", s)
}

pub fn bold(s: &str) -> String {
    wrap("1", s)
}

pub fn red_bold(s: &str) -> String {
    wrap("1;31", s)
}

pub fn green_bold(s: &str) -> String {
    wrap("1;32", s)
}
