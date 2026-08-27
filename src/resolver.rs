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

// ---------------------------------------------------------------------------
// GitHub flake sources (arbitrary repos, not nixpkgs)
// ---------------------------------------------------------------------------

/// Metadata resolved for a single package pulled from an arbitrary GitHub
/// flake, pinned to an exact commit via Nix's own fetcher (`nix flake
/// metadata`) rather than anything nixlayer invents itself.
#[derive(Debug, Clone)]
pub struct GithubPackageInfo {
    pub owner: String,
    pub repo: String,
    pub git_ref: Option<String>,
    pub rev: String,
    pub attr: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
}

/// Resolve `owner/repo[@ref]` + an output attribute into a pinned commit and
/// (best-effort) metadata. Two Nix calls:
///   1. `nix flake metadata` — asks Nix's own fetcher to resolve `ref` (a
///      branch/tag, or the repo's default branch if omitted) to an exact
///      commit. This is the same mechanism `nix flake update` itself uses.
///   2. `nix eval --impure` — evaluates `(builtins.getFlake "github:...@rev").packages.<system>.<attr>.meta`
///      for a description/homepage, if the flake happens to set `meta` at
///      all (many flakes don't; that's fine, fields stay `None`).
/// `--impure` is required here because `builtins.currentSystem` needs it;
/// evaluating a *pinned* rev keeps the actual package resolution pure regardless.
pub fn resolve_github(
    owner: &str,
    repo: &str,
    git_ref: Option<&str>,
    attr: Option<&str>,
) -> Result<GithubPackageInfo> {
    if which("nix").is_none() {
        return Err(NixlayerError::Other(
            "GitHub-sourced packages require the modern `nix` CLI (flakes) — the legacy nix-env fallback doesn't support this.".to_string(),
        ));
    }

    let attr = attr.unwrap_or("default").to_string();
    let flake_url = match git_ref {
        Some(r) => format!("github:{owner}/{repo}/{r}"),
        None => format!("github:{owner}/{repo}"),
    };

    let meta_output = nix_cmd()
        .args(["flake", "metadata", &flake_url, "--json"])
        .output()
        .map_err(|e| NixlayerError::ResolverFailed(e.to_string()))?;
    if !meta_output.status.success() {
        return Err(NixlayerError::ResolverFailed(format!(
            "could not resolve {flake_url} — is the repo public and does it have a flake.nix?\n{}",
            String::from_utf8_lossy(&meta_output.stderr).trim()
        )));
    }
    let meta: FlakeMetadata = serde_json::from_slice(&meta_output.stdout)?;
    let rev = meta.locked.rev.ok_or_else(|| {
        NixlayerError::ResolverFailed(format!(
            "{flake_url} resolved, but Nix didn't report a pinned commit — unexpected flake ref shape"
        ))
    })?;

    let pinned_url = format!("github:{owner}/{repo}/{rev}");
    let expr = format!(
        r#"let f = builtins.getFlake "{pinned_url}"; system = builtins.currentSystem; p = f.packages.${{system}}.{attr}; in {{ description = p.meta.description or null; homepage = p.meta.homepage or null; }}"#
    );
    let eval_output = nix_cmd()
        .args(["eval", "--impure", "--json", "--expr", &expr])
        .output();

    let (description, homepage) = match eval_output {
        Ok(out) if out.status.success() => {
            #[derive(Deserialize)]
            struct Meta {
                description: Option<String>,
                homepage: Option<String>,
            }
            match serde_json::from_slice::<Meta>(&out.stdout) {
                Ok(m) => (m.description, m.homepage),
                Err(_) => (None, None),
            }
        }
        // Missing `packages.<system>.<attr>` output, or the flake sets no
        // meta at all — not fatal, we still have the pinned commit.
        _ => (None, None),
    };

    Ok(GithubPackageInfo {
        owner: owner.to_string(),
        repo: repo.to_string(),
        git_ref: git_ref.map(|s| s.to_string()),
        rev,
        attr,
        description,
        homepage,
    })
}

#[derive(Debug, Deserialize)]
struct FlakeMetadata {
    locked: LockedRef,
}

#[derive(Debug, Deserialize)]
struct LockedRef {
    rev: Option<String>,
}

#[cfg(test)]
mod github_tests {
    use super::*;

    #[test]
    fn eval_expr_has_balanced_braces() {
        let expr = format!(
            r#"let f = builtins.getFlake "github:a/b/1234567"; system = builtins.currentSystem; p = f.packages.${{system}}.default; in {{ description = p.meta.description or null; homepage = p.meta.homepage or null; }}"#
        );
        assert_eq!(expr.matches('{').count(), expr.matches('}').count());
    }
}

// ---------------------------------------------------------------------------
// Imperative profile scanning (nix-env / nix profile), for `nixlayer import profile`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ProfileBackend {
    NixProfile,
    NixEnv,
}

#[derive(Debug, Clone)]
pub struct ProfileEntry {
    /// What to show the person (e.g. "firefox-121.0").
    pub display_name: String,
    /// Best-guess nixpkgs attribute to try resolving.
    pub guessed_attr: String,
    /// Whether this entry's own metadata suggests it came from nixpkgs at all
    /// (vs. some other flake/channel) — only meaningful for the NixProfile backend.
    pub likely_nixpkgs: bool,
    pub backend: ProfileBackend,
    /// The identifier to hand to `nix profile remove` / `nix-env -e`, if the
    /// person confirms they want it removed after a successful import.
    pub removal_key: String,
}

/// Scan whatever's imperatively installed. Tries the modern `nix profile`
/// first (richer: it remembers which flake attribute a package came from);
/// falls back to `nix-env` for older/plain profiles. JSON is parsed loosely
/// (`serde_json::Value`) rather than into strict structs, since the `nix
/// profile list --json` schema has changed across Nix versions and this
/// hasn't been checked against a real installation — see DESIGN.md.
pub fn scan_profile() -> Result<Vec<ProfileEntry>> {
    if which("nix").is_some() {
        if let Some(entries) = try_scan_nix_profile() {
            return Ok(entries);
        }
    }
    scan_nix_env()
}

fn try_scan_nix_profile() -> Option<Vec<ProfileEntry>> {
    let output = nix_cmd().args(["profile", "list", "--json"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    // Schema has varied across Nix releases: sometimes `elements` is an
    // object keyed by name, sometimes an array. Handle both.
    let elements = json.get("elements")?;
    let mut out = Vec::new();

    let mut push_entry = |name_hint: &str, el: &serde_json::Value| {
        let attr_path = el.get("attrPath").and_then(|v| v.as_str()).unwrap_or("");
        let original_url = el
            .get("originalUrl")
            .and_then(|v| v.as_str())
            .or_else(|| el.get("url").and_then(|v| v.as_str()))
            .unwrap_or("");
        let guessed_attr = attr_path
            .rsplit_once('.')
            .map(|(_, a)| a.to_string())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| name_hint.to_string());
        let likely_nixpkgs = original_url.to_lowercase().contains("nixpkgs");
        out.push(ProfileEntry {
            display_name: name_hint.to_string(),
            guessed_attr,
            likely_nixpkgs,
            backend: ProfileBackend::NixProfile,
            removal_key: name_hint.to_string(),
        });
    };

    if let Some(map) = elements.as_object() {
        for (name, el) in map {
            push_entry(name, el);
        }
    } else if let Some(list) = elements.as_array() {
        for el in list {
            let name_hint = el
                .get("attrPath")
                .and_then(|v| v.as_str())
                .and_then(|a| a.rsplit_once('.').map(|(_, x)| x))
                .unwrap_or("unknown")
                .to_string();
            push_entry(&name_hint, el);
        }
    } else {
        return None;
    }

    Some(out)
}

fn scan_nix_env() -> Result<Vec<ProfileEntry>> {
    let output = Command::new("nix-env")
        .args(["-q", "--json"])
        .output()
        .map_err(|e| NixlayerError::ResolverFailed(e.to_string()))?;
    if !output.status.success() {
        return Err(NixlayerError::ResolverFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let raw: std::collections::BTreeMap<String, LegacyEntry> =
        serde_json::from_slice(&output.stdout)?;
    Ok(raw
        .into_iter()
        .map(|(display_name, entry)| ProfileEntry {
            guessed_attr: entry.pname.clone(),
            display_name,
            likely_nixpkgs: true, // nix-env has no other notion of "source" to check
            backend: ProfileBackend::NixEnv,
            removal_key: entry.pname,
        })
        .collect())
}

/// Actually remove an entry from wherever it's imperatively installed. Only
/// called after explicit person confirmation. `nix-env -e` is stable across
/// versions; `nix profile remove <name>` matches modern (2.19+) Nix — older
/// `nix profile` versions may expect a numeric index instead, in which case
/// this fails cleanly and the caller should tell the person to check `nix
/// profile list` themselves.
pub fn remove_from_profile(entry: &ProfileEntry) -> Result<()> {
    let (program, args): (&str, Vec<String>) = match entry.backend {
        ProfileBackend::NixEnv => ("nix-env", vec!["-e".to_string(), entry.removal_key.clone()]),
        ProfileBackend::NixProfile => (
            "nix",
            vec![
                "--extra-experimental-features".to_string(),
                EXPERIMENTAL.to_string(),
                "profile".to_string(),
                "remove".to_string(),
                entry.removal_key.clone(),
            ],
        ),
    };
    let output = Command::new(program)
        .args(&args)
        .output()
        .map_err(|e| NixlayerError::Other(e.to_string()))?;
    if !output.status.success() {
        return Err(NixlayerError::Other(format!(
            "{} failed: {}",
            program,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
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
