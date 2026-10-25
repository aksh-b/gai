//! Terminal output helpers.

use std::io::IsTerminal;

fn enabled() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// ANSI styling, only when stderr is a terminal.
pub fn paint(text: &str, code: &str) -> String {
    if enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

pub fn dim(text: &str) -> String {
    paint(text, "2")
}

pub fn red(text: &str) -> String {
    paint(text, "31")
}

pub fn green(text: &str) -> String {
    paint(text, "32")
}

pub fn yellow(text: &str) -> String {
    paint(text, "33")
}

pub fn cyan(text: &str) -> String {
    paint(text, "36")
}

pub fn bold(text: &str) -> String {
    paint(text, "1")
}

/// First line of `s`, cut to `max` characters.
pub fn one_line(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_owned()
    }
}

pub fn age(t: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - t).num_seconds().max(0);
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}
