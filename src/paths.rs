use std::path::{Path, PathBuf};

use crate::error::{NixlayerError, Result};

/// Everything nixlayer needs to know about where the user's NixOS config lives,
/// and where nixlayer's own owned directory sits inside it.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Root of the NixOS configuration, e.g. /etc/nixos
    pub config_root: PathBuf,
    /// modules/nixlayer inside the config root — the ONLY directory nixlayer writes package data into.
    pub nixlayer_dir: PathBuf,
    /// configuration.nix, if present at the root (best-effort import target).
    pub configuration_nix: Option<PathBuf>,
    /// flake.nix, if present at the root (informational only in v0.1).
    pub flake_nix: Option<PathBuf>,
}

const CONFIG_DIR_ENV: &str = "NIXLAYER_CONFIG_DIR";
const DEFAULT_CONFIG_ROOT: &str = "/etc/nixos";

impl Paths {
    /// Locate the NixOS config root. Honors NIXLAYER_CONFIG_DIR for testing / nonstandard
    /// setups; otherwise defaults to /etc/nixos, which is where NixOS puts it by convention.
    pub fn discover() -> Result<Paths> {
        let root = if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
            PathBuf::from(dir)
        } else {
            PathBuf::from(DEFAULT_CONFIG_ROOT)
        };

        if !root.is_dir() {
            return Err(NixlayerError::ConfigNotFound(root));
        }

        Ok(Paths::at(root))
    }

    pub fn at(root: PathBuf) -> Paths {
        let nixlayer_dir = root.join("modules").join("nixlayer");
        let configuration_nix = existing(root.join("configuration.nix"));
        let flake_nix = existing(root.join("flake.nix"));
        Paths {
            config_root: root,
            nixlayer_dir,
            configuration_nix,
            flake_nix,
        }
    }

    pub fn default_nix(&self) -> PathBuf {
        self.nixlayer_dir.join("default.nix")
    }

    pub fn state_file(&self) -> PathBuf {
        self.nixlayer_dir.join(".nixlayer-state.json")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.nixlayer_dir.join(".nixlayer-settings.json")
    }

    pub fn category_file(&self, category: &str) -> PathBuf {
        self.nixlayer_dir.join(format!("{category}.nix"))
    }

    pub fn is_initialized(&self) -> bool {
        self.nixlayer_dir.is_dir() && self.default_nix().is_file()
    }

    pub fn require_initialized(&self) -> Result<()> {
        if !self.is_initialized() {
            return Err(NixlayerError::NotInitialized(self.nixlayer_dir.clone()));
        }
        Ok(())
    }

    /// List category names (file stem of every *.nix file in nixlayer_dir except default.nix).
    pub fn list_categories(&self) -> Result<Vec<String>> {
        let mut cats = Vec::new();
        if !self.nixlayer_dir.is_dir() {
            return Ok(cats);
        }
        for entry in std::fs::read_dir(&self.nixlayer_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == "default.nix" || !name.ends_with(".nix") {
                continue;
            }
            cats.push(name.trim_end_matches(".nix").to_string());
        }
        cats.sort();
        Ok(cats)
    }
}

fn existing(p: PathBuf) -> Option<PathBuf> {
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// A timestamped backup path next to `original`, e.g. configuration.nix.bak-20260823-141501
pub fn backup_path(original: &Path) -> PathBuf {
    let ts = timestamp();
    let file_name = original
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    original.with_file_name(format!("{file_name}.bak-{ts}"))
}

fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Not calendar-pretty, but monotonic, sortable, and dependency-free.
    format!("{secs}")
}
