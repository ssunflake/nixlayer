//! A tiny settings file nixlayer owns (`.nixlayer-settings.json`), separate
//! from `.nixlayer-state.json` (which is a rebuild snapshot, not config).
//! Currently holds exactly one flag: whether `nixpkgs.config.allowUnfree` is
//! turned on. This is written into `modules/nixlayer/default.nix` itself when
//! enabled — nixlayer already owns and regenerates that file, so toggling
//! unfree support never requires touching `configuration.nix` at all.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::paths::Paths;

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Settings {
    #[serde(default)]
    pub allow_unfree: bool,
}

impl Settings {
    pub fn load(paths: &Paths) -> Result<Settings> {
        let path = paths.settings_file();
        if !path.is_file() {
            return Ok(Settings::default());
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw).unwrap_or_default())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let path = paths.settings_file();
        let raw = serde_json::to_string_pretty(self)?;
        fs::write(path, raw)?;
        Ok(())
    }
}

/// Apply the current setting to `modules/nixlayer/default.nix` and write it.
/// Safe to call any time default.nix would otherwise be regenerated.
pub fn sync_default_nix(paths: &Paths, settings: &Settings) -> Result<()> {
    let rendered = crate::default_nix::render(settings.allow_unfree);
    fs::write(paths.default_nix(), rendered)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_unfree_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().to_path_buf());
        let settings = Settings::load(&paths).unwrap();
        assert!(!settings.allow_unfree);
    }

    #[test]
    fn roundtrips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("modules/nixlayer")).unwrap();
        let paths = Paths::at(dir.path().to_path_buf());
        let mut settings = Settings::load(&paths).unwrap();
        settings.allow_unfree = true;
        settings.save(&paths).unwrap();

        let reloaded = Settings::load(&paths).unwrap();
        assert!(reloaded.allow_unfree);
    }
}
