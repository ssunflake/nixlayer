//! Minimal, deliberately dumb Nix-text helpers.
//!
//! nixlayer does NOT contain a general Nix parser. It only ever needs to do two
//! narrow, well-defined things:
//!   1. Find a `[ ... ]` list following some marker text and read/replace its contents.
//!   2. Validate that a file it just wrote is syntactically valid Nix.
//!
//! Anything more exotic than that is treated as "I don't understand this file"
//! and nixlayer refuses to touch it automatically. This is intentional: guessing
//! wrong on someone's NixOS config is much worse than stopping and asking.

use std::path::Path;
use std::process::Command;

use crate::error::{NixlayerError, Result};

/// A `[...]` block found in a larger text, with byte offsets of the brackets
/// (inclusive of `[` and `]`) and the raw inner text between them.
pub struct BracketBlock {
    pub open_idx: usize,
    pub close_idx: usize,
    pub inner: String,
}

/// Starting the scan at `search_from`, find the first `[` and its matching `]`
/// (bracket-depth aware, so nested lists don't confuse it).
pub fn find_bracket_block(text: &str, search_from: usize) -> Option<BracketBlock> {
    let bytes = text.as_bytes();
    let open_idx = text[search_from..].find('[')? + search_from;
    let mut depth = 0i32;
    let mut i = open_idx;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    let inner = text[open_idx + 1..i].to_string();
                    return Some(BracketBlock {
                        open_idx,
                        close_idx: i,
                        inner,
                    });
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Replace the contents of a previously-found bracket block in `text`, keeping
/// everything else byte-for-byte identical.
pub fn replace_bracket_inner(text: &str, block: &BracketBlock, new_inner: &str) -> String {
    let mut out = String::with_capacity(text.len() + new_inner.len());
    out.push_str(&text[..block.open_idx + 1]);
    out.push_str(new_inner);
    out.push_str(&text[block.close_idx..]);
    out
}

/// Tokens allowed as bare package/attribute-path entries, e.g. `firefox` or
/// `nodePackages.typescript`. Anything else in a managed list is a sign the
/// file has custom content nixlayer shouldn't touch.
pub fn is_simple_attr_path(tok: &str) -> bool {
    if tok.is_empty() {
        return false;
    }
    tok.split('.').all(|seg| {
        let mut chars = seg.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '\'' || c == '-')
    })
}

/// Split the inner text of a package list into tokens, stripping `#` line comments.
/// Returns Err with the offending tokens if anything isn't a simple attribute path.
pub fn tokenize_package_list(inner: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut bad = Vec::new();
    for raw_line in inner.lines() {
        let line = match raw_line.find('#') {
            Some(idx) => &raw_line[..idx],
            None => raw_line,
        };
        for tok in line.split_whitespace() {
            let tok = tok.trim_end_matches(',');
            if tok.is_empty() {
                continue;
            }
            if is_simple_attr_path(tok) {
                tokens.push(tok.to_string());
            } else {
                bad.push(tok.to_string());
            }
        }
    }
    if !bad.is_empty() {
        return Err(NixlayerError::Other(bad.join(", ")));
    }
    Ok(tokens)
}

/// Validate Nix syntax using `nix-instantiate --parse`, if available. This is a
/// real parse by the actual Nix implementation, not a homemade check. If neither
/// `nix-instantiate` nor `nix` is present, validation is skipped (caller decides
/// whether that's acceptable) — nixlayer never invents its own Nix parser.
pub fn validate_syntax(path: &Path) -> SyntaxCheck {
    if let Some(out) = try_parse_with(&["nix-instantiate", "--parse"], path) {
        return out;
    }
    if let Some(out) = try_parse_with(
        &["nix", "--extra-experimental-features", "nix-command"],
        path,
    ) {
        // `nix eval --file <path>` at least forces parsing; a parse error surfaces
        // as a nonzero exit with a clear message, which is what we care about here.
        return out;
    }
    SyntaxCheck::Skipped
}

fn try_parse_with(prefix: &[&str], path: &Path) -> Option<SyntaxCheck> {
    let program = prefix[0];
    if which(program).is_none() {
        return None;
    }
    let mut cmd = Command::new(program);
    cmd.args(&prefix[1..]);
    // For the `nix` fallback we do `nix eval --file <path> --apply 'x: null'`
    // to force a parse without evaluating systemPackages against real pkgs.
    if program == "nix" {
        cmd.args(["eval", "--file"])
            .arg(path)
            .args(["--apply", "x: null", "--json"]);
    } else {
        cmd.arg(path);
    }
    let output = cmd.output().ok()?;
    if output.status.success() {
        Some(SyntaxCheck::Ok)
    } else {
        Some(SyntaxCheck::Invalid(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

pub enum SyntaxCheck {
    Ok,
    Invalid(String),
    Skipped,
}

pub fn which(program: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).find_map(|dir| {
        let full = dir.join(program);
        if full.is_file() {
            Some(full)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_simple_bracket_block() {
        let text = "foo = with pkgs; [\n  a\n  b\n];";
        let block = find_bracket_block(text, 0).unwrap();
        assert_eq!(block.inner.trim(), "a\n  b");
    }

    #[test]
    fn handles_nested_brackets() {
        let text = "x = [ [ a b ] c ];";
        let block = find_bracket_block(text, 0).unwrap();
        assert_eq!(block.inner.trim(), "[ a b ] c");
    }

    #[test]
    fn tokenizes_and_strips_comments() {
        let inner = "  firefox\n  # a comment\n  vlc\n  nodePackages.typescript\n";
        let toks = tokenize_package_list(inner).unwrap();
        assert_eq!(toks, vec!["firefox", "vlc", "nodePackages.typescript"]);
    }

    #[test]
    fn rejects_complex_entries() {
        let inner = "  firefox\n  (python3.withPackages (p: [ p.numpy ]))\n";
        let err = tokenize_package_list(inner).unwrap_err();
        match err {
            NixlayerError::Other(s) => assert!(s.contains("withPackages") || s.contains("(")),
            _ => panic!("wrong error type"),
        }
    }

    #[test]
    fn replace_preserves_surrounding_text() {
        let text = "before [ a b ] after";
        let block = find_bracket_block(text, 0).unwrap();
        let replaced = replace_bracket_inner(text, &block, " c d ");
        assert_eq!(replaced, "before [ c d ] after");
    }
}
