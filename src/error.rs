use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NixlayerError {
    #[error("could not find a NixOS configuration.\n  Looked in: {0}\n  Set NIXLAYER_CONFIG_DIR to point at your NixOS config directory if it lives elsewhere.")]
    ConfigNotFound(PathBuf),

    #[error("nixlayer has not been initialized here yet.\n  Expected: {0}\n  Run `nixlayer init` first.")]
    NotInitialized(PathBuf),

    #[error("nixlayer is already initialized at {0}")]
    AlreadyInitialized(PathBuf),

    #[error("category '{0}' does not exist.\n  Known categories: {1}\n  Create it with `nixlayer add <package> --category {0}`.")]
    UnknownCategory(String, String),

    #[error("package '{0}' not found in any nixlayer category.\n  Run `nixlayer list` to see what's managed, or `nixlayer search {0}` to look it up in nixpkgs.")]
    PackageNotManaged(String),

    #[error("package '{0}' is already declared in category '{1}'.\n  Use `nixlayer move {0} <category>` to relocate it instead.")]
    PackageAlreadyExists(String, String),

    #[error("refusing to auto-manage {0}: it contains entries nixlayer doesn't recognize as plain package names ({1}).\n  Edit this file by hand, or move the custom entries elsewhere.")]
    UnparseableCategoryFile(PathBuf, String),

    #[error("duplicate package declaration:\n  '{0}' appears in more than one category: {1}\n  Resolve this before rebuilding, e.g. `nixlayer move {0} <category>`.")]
    DuplicatePackage(String, String),

    #[error("could not safely locate an `imports = [ ... ];` block in {0}.\n  nixlayer only edits files it fully understands, and won't guess here.\n  Add this line yourself inside your imports list:\n\n      ./modules/nixlayer/default.nix\n")]
    UnsafeConfigEdit(PathBuf),

    #[error("'{0}' does not look like a plain nixpkgs attribute name or path (e.g. 'firefox', 'nodePackages.typescript').")]
    InvalidAttributeName(String),

    #[error("nix package search/eval failed: {0}\n  This usually means either:\n    - `nix` isn't installed or isn't on PATH, or\n    - the `nixpkgs` flake registry entry can't be fetched (no network / no registry), or\n    - the package name genuinely doesn't exist in nixpkgs.\n  Run `nixlayer doctor` for a full diagnosis.")]
    ResolverFailed(String),

    #[error("no nix package manager found on this system (checked for `nix`, `nix-env`, `nix-instantiate`).\n  nixlayer needs Nix to search and validate packages. Are you running this inside a NixOS/Nix environment?")]
    NoNixFound,

    #[error("nixos-rebuild failed (exit code {0}). See output above.\n  The previous generation is still active; nothing was left half-broken.")]
    RebuildFailed(i32),

    #[error("refusing to rebuild: {0}\n  Run `nixlayer doctor` for details, or `nixlayer diff` to see what would change.")]
    UnsafeToRebuild(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, NixlayerError>;
