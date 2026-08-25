//! Resolves user-typed package queries into real nixpkgs attribute names,
//! using Nix's own tooling rather than any nixlayer-side package database.
//!
//! Two backends, tried in order:
//!
//! 1. **Flakes** (`nix search` / `nix eval` against the `nixpkgs` flake
//!    registry entry). This is the modern, structured, JSON-friendly path and
//!    is preferred when available.
//! 2. **Legacy** (`nix-env -qaP --json`) for systems without flakes enabled.
//!    Gives reduced metadata (no license/homepage) but still resolves real
//!    attribute names against the channel-based nixpkgs.
//!
//! Known limitation (v0.1, documented rather than hidden): the search/eval
//! above resolves against whatever nixpkgs the `nixpkgs` flake registry entry
//! or active channel points to on this machine — which may not be byte-for-byte
//! the same nixpkgs your own flake.nix pins. That's fine for finding the right
//! *attribute name* (nixpkgs attribute names are extremely stable), but exact
//! version/description shown by `search`/`info` can drift slightly from what
//! actually gets built. The final build always goes through the user's own
//! `pkgs` at `nixos-rebuild` time — nixlayer never bypasses that.

use std::process::Command;

use serde::Deserialize;

use crate::error::{NixlayerError, Result};
use crate::nixfile::{is_simple_attr_path, which};

#[derive(Debug, Clone)]
pub enum Backend {
    Flakes,
    Legacy,
}

#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub attribute: String,
    pub pname: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Vec<String>,
    pub free: bool,
    pub broken: bool,
    pub source: Backend,
}

const EXPERIMENTAL: &str = "nix-command flakes";

pub fn detect_backend() -> Result<Backend> {
    if which("nix").is_some() {
        Ok(Backend::Flakes)
    } else if which("nix-env").is_some() {
        Ok(Backend::Legacy)
    } else {
        Err(NixlayerError::NoNixFound)
    }
}

/// Resolve a single exact attribute name to full metadata. This is the fast
/// path used by `add <exact-attribute>` and `info <exact-attribute>`.
pub fn resolve_attribute(attr: &str) -> Result<PackageInfo> {
    if !is_simple_attr_path(attr) {
        return Err(NixlayerError::InvalidAttributeName(attr.to_string()));
    }
    match detect_backend()? {
        Backend::Flakes => resolve_attribute_flakes(attr),
        Backend::Legacy => resolve_attribute_legacy(attr),
    }
}

/// Search nixpkgs for a free-text query, returning candidate packages ranked
/// by relevance (as Nix's own search gives them to us — nixlayer does no
/// re-ranking of its own).
pub fn search(query: &str) -> Result<Vec<PackageInfo>> {
    match detect_backend()? {
        Backend::Flakes => search_flakes(query),
        Backend::Legacy => search_legacy(query),
    }
}

// ---------------------------------------------------------------------------
// Flakes backend
// ---------------------------------------------------------------------------

fn nix_cmd() -> Command {
    let mut cmd = Command::new("nix");
    cmd.args(["--extra-experimental-features", EXPERIMENTAL]);
    cmd
}

fn resolve_attribute_flakes(attr: &str) -> Result<PackageInfo> {
    let expr = eval_apply_expr();
    let flake_ref = format!("nixpkgs#{attr}");
    let output = nix_cmd()
        .args(["eval", "--json", &flake_ref, "--apply", &expr])
        .output()
        .map_err(|e| NixlayerError::ResolverFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(NixlayerError::ResolverFailed(format!(
            "attribute '{attr}' not found in nixpkgs (via flake registry).\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let raw: RawMeta = serde_json::from_slice(&output.stdout)?;
    Ok(raw.into_info(attr))
}

fn search_flakes(query: &str) -> Result<Vec<PackageInfo>> {
    let output = nix_cmd()
        .args(["search", "nixpkgs", query, "--json"])
        .output()
        .map_err(|e| NixlayerError::ResolverFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(NixlayerError::ResolverFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    // `nix search --json` returns { "legacyPackages.<system>.<attr>": {pname, version, description} }
    let raw: std::collections::BTreeMap<String, SearchEntry> =
        serde_json::from_slice(&output.stdout)?;

    let mut results = Vec::new();
    for (key, entry) in raw {
        let attr = key
            .rsplit_once('.')
            .map(|(_, a)| a.to_string())
            .unwrap_or(key);
        results.push(PackageInfo {
            attribute: attr,
            pname: Some(entry.pname),
            version: Some(entry.version),
            description: if entry.description.is_empty() {
                None
            } else {
                Some(entry.description)
            },
            homepage: None,
            license: Vec::new(),
            free: true, // unknown until `info`/`add` does a full eval
            broken: false,
            source: Backend::Flakes,
        });
    }
    Ok(results)
}

/// A `nix eval --apply` expression that reduces a derivation to a small,
/// serializable metadata record. Every field uses `or null`/`or true` so it
/// degrades gracefully across nixpkgs versions instead of throwing.
fn eval_apply_expr() -> String {
    r#"
    p: {
      pname = p.pname or (p.name or null);
      version = p.version or null;
      description = p.meta.description or null;
      homepage = p.meta.homepage or null;
      broken = p.meta.broken or false;
      license =
        let l = p.meta.license or null; in
        if l == null then []
        else if builtins.isList l then
          map (x: if builtins.isString x then x else (x.spdxId or x.shortName or x.fullName or "unknown")) l
        else if builtins.isString l then [ l ]
        else [ (l.spdxId or l.shortName or l.fullName or "unknown") ];
      free =
        let l = p.meta.license or null; in
        if l == null then true
        else if builtins.isList l then builtins.all (x: if builtins.isString x then true else (x.free or true)) l
        else if builtins.isString l then true
        else l.free or true;
    }
    "#
    .trim()
    .to_string()
}

#[derive(Debug, Deserialize)]
struct RawMeta {
    pname: Option<String>,
    version: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    #[serde(default)]
    broken: bool,
    #[serde(default)]
    license: Vec<String>,
    #[serde(default = "default_true")]
    free: bool,
}

fn default_true() -> bool {
    true
}

impl RawMeta {
    fn into_info(self, attr: &str) -> PackageInfo {
        PackageInfo {
            attribute: attr.to_string(),
            pname: self.pname,
            version: self.version,
            description: self.description,
            homepage: self.homepage,
            license: self.license,
            free: self.free,
            broken: self.broken,
            source: Backend::Flakes,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SearchEntry {
    #[serde(default)]
    pname: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
}

// ---------------------------------------------------------------------------
// Legacy backend (nix-env, channels, no flakes)
// ---------------------------------------------------------------------------

fn resolve_attribute_legacy(attr: &str) -> Result<PackageInfo> {
    let output = Command::new("nix-env")
        .args(["-qa", "--json", "-A", &format!("nixpkgs.{attr}")])
        .output()
        .map_err(|e| NixlayerError::ResolverFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(NixlayerError::ResolverFailed(format!(
            "attribute '{attr}' not found in nixpkgs (via nix-env, legacy mode).\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let raw: std::collections::BTreeMap<String, LegacyEntry> =
        serde_json::from_slice(&output.stdout)?;
    let entry = raw
        .into_values()
        .next()
        .ok_or_else(|| NixlayerError::ResolverFailed(format!("no match for '{attr}'")))?;

    Ok(PackageInfo {
        attribute: attr.to_string(),
        pname: Some(entry.pname),
        version: Some(entry.version.unwrap_or_default()),
        description: None,
        homepage: None,
        license: Vec::new(),
        free: true,
        broken: false,
        source: Backend::Legacy,
    })
}

fn search_legacy(query: &str) -> Result<Vec<PackageInfo>> {
    let pattern = format!(".*{}.*", regex_escape(query));
    let output = Command::new("nix-env")
        .args(["-qaP", "--json", &pattern])
        .output()
        .map_err(|e| NixlayerError::ResolverFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(NixlayerError::ResolverFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let raw: std::collections::BTreeMap<String, LegacyEntry> =
        serde_json::from_slice(&output.stdout)?;

    let mut results = Vec::new();
    for (attr_path, entry) in raw {
        // attr_path looks like "nixpkgs.steam" or "nixos.steam"
        let attr = attr_path
            .rsplit_once('.')
            .map(|(_, a)| a.to_string())
            .unwrap_or(attr_path);
        results.push(PackageInfo {
            attribute: attr,
            pname: Some(entry.pname),
            version: entry.version,
            description: None,
            homepage: None,
            license: Vec::new(),
            free: true,
            broken: false,
            source: Backend::Legacy,
        });
    }
    Ok(results)
}

fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[derive(Debug, Deserialize)]
struct LegacyEntry {
    #[serde(default)]
    pname: String,
    #[serde(default)]
    version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_expr_is_well_formed_nix_ish() {
        // Cheap sanity check without invoking nix: braces balance.
        let expr = eval_apply_expr();
        let opens = expr.matches('{').count();
        let closes = expr.matches('}').count();
        assert_eq!(opens, closes);
    }

    #[test]
    fn regex_escape_handles_specials() {
        assert_eq!(regex_escape("c++"), "c\\+\\+");
        assert_eq!(regex_escape("steam"), "steam");
    }
}
