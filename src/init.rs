use std::fs;
use std::path::Path;

use crate::category::{Category, DEFAULT_CATEGORY};
use crate::default_nix;
use crate::error::{NixlayerError, Result};
use crate::nixfile::{find_bracket_block, replace_bracket_inner};
use crate::paths::{backup_path, Paths};

const IMPORT_LINE: &str = "./modules/nixlayer/default.nix";
const IMPORT_MARKER: &str = "imports";

pub struct InitReport {
    pub created_dir: bool,
    pub created_default_nix: bool,
    pub created_default_category: bool,
    pub configuration_nix_patched: bool,
    pub configuration_nix_already_ok: bool,
    pub manual_step_needed: Option<String>,
}

pub fn run(dry_run: bool) -> Result<InitReport> {
    let paths = Paths::discover()?;

    if paths.is_initialized() {
        return Err(NixlayerError::AlreadyInitialized(paths.nixlayer_dir.clone()));
    }

    let created_dir = !paths.nixlayer_dir.is_dir();
    let created_default_nix = !paths.default_nix().is_file();
    let default_category_path = paths.category_file(DEFAULT_CATEGORY);
    let created_default_category = !default_category_path.is_file();

    if !dry_run {
        fs::create_dir_all(&paths.nixlayer_dir)?;
        fs::write(paths.default_nix(), default_nix::render(false))?;
        if created_default_category {
            fs::write(&default_category_path, Category::template(DEFAULT_CATEGORY))?;
        }
    }

    let (patched, already_ok, manual_step) = match &paths.configuration_nix {
        Some(config_path) => patch_configuration(config_path, dry_run)?,
        None => (
            false,
            false,
            Some(format!(
                "No configuration.nix found at {}. Add this import to whichever module \
                 makes up your system config:\n\n    {}\n",
                paths.config_root.display(),
                IMPORT_LINE
            )),
        ),
    };

    Ok(InitReport {
        created_dir,
        created_default_nix,
        created_default_category,
        configuration_nix_patched: patched,
        configuration_nix_already_ok: already_ok,
        manual_step_needed: manual_step,
    })
}

/// Try to add `./modules/nixlayer/default.nix` to configuration.nix's `imports = [ ... ];`
/// list. Only ever touches that one list; backs up the file first; if it can't
/// confidently find a single unambiguous imports list, it stops and explains
/// the one line the user needs to add themselves.
fn patch_configuration(path: &Path, dry_run: bool) -> Result<(bool, bool, Option<String>)> {
    let text = fs::read_to_string(path)?;

    if text.contains(IMPORT_LINE) {
        return Ok((false, true, None));
    }

    let Some(marker_idx) = find_imports_marker(&text) else {
        return Ok((
            false,
            false,
            Some(manual_instructions(path)),
        ));
    };

    let Some(block) = find_bracket_block(&text, marker_idx) else {
        return Ok((false, false, Some(manual_instructions(path))));
    };

    // Refuse if there's a second top-level `imports = [` — ambiguous, don't guess.
    if find_imports_marker(&text[block.close_idx..]).is_some() {
        return Ok((
            false,
            false,
            Some(format!(
                "{} contains more than one `imports = [ ... ];` block, so nixlayer \
                 can't tell which one is authoritative. Add this line to the right one yourself:\n\n    {}\n",
                path.display(),
                IMPORT_LINE
            )),
        ));
    }

    // Preserve whatever trailing whitespace/indentation already sat before the
    // closing `]` (e.g. "\n  "), so the bracket doesn't end up jammed against
    // the last line after our insertion.
    let trimmed_end = block.inner.trim_end_matches([' ', '\t', '\n']);
    let trailing_ws = &block.inner[trimmed_end.len()..];
    let trailing_ws = if trailing_ws.is_empty() {
        "\n"
    } else {
        trailing_ws
    };

    let mut new_inner = trimmed_end.to_string();
    if !new_inner.is_empty() {
        new_inner.push('\n');
    }
    new_inner.push_str("    ");
    new_inner.push_str(IMPORT_LINE);
    new_inner.push_str(trailing_ws);

    let new_text = replace_bracket_inner(&text, &block, &new_inner);

    if dry_run {
        return Ok((true, false, None));
    }

    fs::write(backup_path(path), &text)?;
    fs::write(path, &new_text)?;

    if let crate::nixfile::SyntaxCheck::Invalid(msg) = crate::nixfile::validate_syntax(path) {
        // Roll back immediately — never leave a broken configuration.nix behind.
        fs::write(path, &text)?;
        return Err(NixlayerError::Other(format!(
            "patching {} would have produced invalid Nix syntax, so nixlayer reverted it:\n{msg}\n\n\
             Please add this line to your imports list by hand:\n\n    {}\n",
            path.display(),
            IMPORT_LINE
        )));
    }

    Ok((true, false, None))
}

fn find_imports_marker(text: &str) -> Option<usize> {
    // Look for a line that is (ignoring leading whitespace) `imports = [` or `imports=[`
    // to avoid matching the word "imports" inside comments or strings elsewhere.
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(IMPORT_MARKER) {
        let idx = search_from + rel;
        let line_start = text[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let prefix = text[line_start..idx].trim();
        if prefix.is_empty() {
            let after = text[idx + IMPORT_MARKER.len()..].trim_start();
            if after.starts_with('=') {
                return Some(idx);
            }
        }
        search_from = idx + IMPORT_MARKER.len();
    }
    None
}

fn manual_instructions(path: &Path) -> String {
    format!(
        "Could not confidently find an `imports = [ ... ];` list in {}.\n\
         Add this line to your imports list by hand:\n\n    {}\n",
        path.display(),
        IMPORT_LINE
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_tmp(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("configuration.nix");
        fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn patches_simple_imports_list() {
        let (_dir, path) = write_tmp(
            "{ config, pkgs, ... }:\n{\n  imports = [\n    ./hardware-configuration.nix\n  ];\n\n  networking.hostName = \"box\";\n}\n",
        );
        let (patched, already_ok, manual) = patch_configuration(&path, false).unwrap();
        assert!(patched);
        assert!(!already_ok);
        assert!(manual.is_none());
        let out = fs::read_to_string(&path).unwrap();
        assert!(out.contains("./hardware-configuration.nix"));
        assert!(out.contains(IMPORT_LINE));
        assert!(out.contains("networking.hostName"));
    }

    #[test]
    fn idempotent_if_already_present() {
        let (_dir, path) = write_tmp(&format!(
            "{{ ... }}:\n{{\n  imports = [\n    {}\n  ];\n}}\n",
            IMPORT_LINE
        ));
        let before = fs::read_to_string(&path).unwrap();
        let (patched, already_ok, _) = patch_configuration(&path, false).unwrap();
        assert!(!patched);
        assert!(already_ok);
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn refuses_ambiguous_double_imports() {
        let (_dir, path) = write_tmp(
            "{ ... }:\n{\n  imports = [ ./a.nix ];\n}\n# oops two files concatenated\n{ ... }:\n{\n  imports = [ ./b.nix ];\n}\n",
        );
        let before = fs::read_to_string(&path).unwrap();
        let (patched, already_ok, manual) = patch_configuration(&path, false).unwrap();
        assert!(!patched);
        assert!(!already_ok);
        assert!(manual.is_some());
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after, "ambiguous file must be left untouched");
    }

    #[test]
    fn dry_run_does_not_write() {
        let (_dir, path) = write_tmp("{ ... }:\n{\n  imports = [\n  ];\n}\n");
        let before = fs::read_to_string(&path).unwrap();
        let (patched, _already_ok, _manual) = patch_configuration(&path, true).unwrap();
        assert!(patched);
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after, "dry run must not write");
    }

    #[test]
    fn creates_backup_on_write() {
        let (dir, path) = write_tmp("{ ... }:\n{\n  imports = [\n  ];\n}\n");
        patch_configuration(&path, false).unwrap();
        let has_backup = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".bak-"));
        assert!(has_backup, "expected a .bak- file next to configuration.nix");
    }
}
