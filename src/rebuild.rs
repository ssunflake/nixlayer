use std::process::Command;

use crate::category;
use crate::error::{NixlayerError, Result};
use crate::nixfile::{which, SyntaxCheck};
use crate::paths::Paths;
use crate::state::State;

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Switch,
    Boot,
    Test,
}

impl Mode {
    pub fn as_arg(self) -> &'static str {
        match self {
            Mode::Switch => "switch",
            Mode::Boot => "boot",
            Mode::Test => "test",
        }
    }
}

/// Everything checked before nixlayer will let a rebuild proceed. Returns Err
/// with a clear, actionable message if anything is unsafe; never partially
/// applies a rebuild on a config it knows is broken.
pub fn validate(paths: &Paths) -> Result<()> {
    paths.require_initialized()?;

    // Duplicate declarations must be resolved first.
    let dups = category::find_duplicates(paths)?;
    if !dups.is_empty() {
        let (pkg, cats) = &dups[0];
        return Err(NixlayerError::UnsafeToRebuild(format!(
            "duplicate package declaration: '{pkg}' appears in {} category files ({}).",
            cats.len(),
            cats.join(", ")
        )));
    }

    // Every category file must parse cleanly.
    for (name, result) in category::load_all(paths)? {
        if let Err(e) = result {
            return Err(NixlayerError::UnsafeToRebuild(format!(
                "category '{name}' failed to parse: {e}"
            )));
        }
    }

    // Syntax-validate default.nix and every category file, if a Nix parser is available.
    let mut to_check = vec![paths.default_nix()];
    for name in paths.list_categories()? {
        to_check.push(paths.category_file(&name));
    }
    for path in to_check {
        if let SyntaxCheck::Invalid(msg) = crate::nixfile::validate_syntax(&path) {
            return Err(NixlayerError::UnsafeToRebuild(format!(
                "{} has invalid Nix syntax:\n{msg}",
                path.display()
            )));
        }
    }

    Ok(())
}

pub fn run(mode: Mode, dry_run: bool) -> Result<()> {
    let paths = Paths::discover()?;
    validate(&paths)?;

    if dry_run {
        return Ok(());
    }

    if which("nixos-rebuild").is_none() {
        return Err(NixlayerError::Other(
            "`nixos-rebuild` not found on PATH. nixlayer drives the system's own rebuild tool rather \
             than reimplementing it — install/enable it, or run this on an actual NixOS system."
                .to_string(),
        ));
    }

    let status = Command::new("nixos-rebuild")
        .arg(mode.as_arg())
        .status()
        .map_err(|e| NixlayerError::Other(format!("failed to launch nixos-rebuild: {e}")))?;

    if !status.success() {
        return Err(NixlayerError::RebuildFailed(status.code().unwrap_or(-1)));
    }

    // Only snapshot state after a genuinely successful rebuild.
    let snapshot = State::capture_current(&paths)?;
    snapshot.save(&paths)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::Category;

    fn setup() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("NIXLAYER_CONFIG_DIR", dir.path());
        let paths = Paths::at(dir.path().to_path_buf());
        std::fs::create_dir_all(&paths.nixlayer_dir).unwrap();
        std::fs::write(paths.default_nix(), crate::default_nix::render()).unwrap();
        (dir, paths)
    }

    #[test]
    fn validate_fails_on_duplicates() {
        let (_dir, paths) = setup();
        let mut a = Category::new_empty("app", paths.category_file("app"));
        a.add("steam");
        a.write().unwrap();
        let mut g = Category::new_empty("gaming", paths.category_file("gaming"));
        g.add("steam");
        g.write().unwrap();

        let err = validate(&paths).unwrap_err();
        assert!(matches!(err, NixlayerError::UnsafeToRebuild(_)));
    }

    #[test]
    fn validate_passes_clean_setup() {
        let (_dir, paths) = setup();
        let mut a = Category::new_empty("app", paths.category_file("app"));
        a.add("firefox");
        a.write().unwrap();

        assert!(validate(&paths).is_ok());
    }
}
