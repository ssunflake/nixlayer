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
