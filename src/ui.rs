// Minimal ANSI helpers. No dependency on a color crate for something this small.
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

fn colors_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

pub fn ok(msg: &str) {
    if colors_enabled() {
        println!("{GREEN}✓{RESET} {msg}");
    } else {
        println!("[ok] {msg}");
    }
}

pub fn warn(msg: &str) {
    if colors_enabled() {
        println!("{YELLOW}!{RESET} {msg}");
    } else {
        println!("[warn] {msg}");
    }
}

pub fn error(msg: &str) {
    if colors_enabled() {
        eprintln!("{RED}✗{RESET} {msg}");
    } else {
        eprintln!("[error] {msg}");
    }
}

pub fn bold(msg: &str) -> String {
    if colors_enabled() {
        format!("{BOLD}{msg}{RESET}")
    } else {
        msg.to_string()
    }
}

pub fn dim(msg: &str) -> String {
    if colors_enabled() {
        format!("{DIM}{msg}{RESET}")
    } else {
        msg.to_string()
    }
}

pub fn heading(msg: &str) {
    println!("{}", bold(msg));
}

/// A blocking y/N prompt on stdin. Defaults to "no" on empty input or any
/// input that isn't clearly "yes" — used only before genuinely destructive
/// actions (e.g. removing packages from nix-env/profile after importing them).
pub fn confirm(prompt: &str) -> bool {
    use std::io::{self, Write};
    print!("{prompt} [y/N] ");
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Ask the person to pick one of `max` numbered options (1-indexed). Returns
/// None on empty input, EOF (non-interactive/piped stdin), or an out-of-range
/// answer — always treated as "cancel," never a guess.
pub fn prompt_choice(max: usize) -> Option<usize> {
    use std::io::{self, Write};
    print!("Which one? [1-{max}, Enter to cancel] ");
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return None;
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1 && *n <= max)
        .map(|n| n - 1)
}
