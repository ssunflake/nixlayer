use std::fs;
use std::path::PathBuf;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::error::{NixlayerError, Result};
use crate::nixfile::{find_bracket_block, find_brace_block, replace_bracket_inner, tokenize_package_list};
use crate::paths::Paths;

pub const DEFAULT_CATEGORY: &str = "app";
const MARKER: &str = "environment.systemPackages";
const GH_MARKER: &str = "ghPkgs = ";
const GH_SUFFIX: &str = " ++ (builtins.attrValues ghPkgs)";

static GH_ENTRY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"^([A-Za-z_][A-Za-zA-Z0-9_'-]*)\s*=\s*\(builtins\.getFlake\s+"([^"]+)"\)\.packages\.\$\{pkgs\.system\}\.([A-Za-z_][A-Za-zA-Z0-9_.'-]*)\s*;$"#,
    )
    .unwrap()
});
static FLAKE_REF_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^github:([^/]+)/([^/]+)/([0-9a-fA-F]{7,40})$").unwrap());
static SOURCE_REF_COMMENT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^#\s*nixlayer-source-ref:\s*(\S+)$").unwrap());

/// A single package pulled from an arbitrary GitHub flake, pinned to an exact
/// commit for reproducibility. Lives inside a category file's `ghPkgs = { ... }`
/// block rather than as a bare identifier, since it needs to carry the repo +
/// commit + output attribute along with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubSource {
    /// The local name used as `ghPkgs.<name>` and everywhere in the CLI (list/remove/update).
    pub name: String,
    pub owner: String,
    pub repo: String,
    /// Exact pinned commit — always a full/short hex sha, never a branch name.
    pub rev: String,
    /// The branch/tag originally requested (if any), remembered in a comment
    /// so `nixlayer github update` knows what to re-resolve against.
    pub git_ref: Option<String>,
    /// Flake output attribute, e.g. "default" or "packages.foo".
    pub attr: String,
}

impl GithubSource {
    pub fn flake_ref(&self) -> String {
        format!("github:{}/{}/{}", self.owner, self.repo, self.rev)
    }
}

/// Either kind of thing a category can hold, used by generic operations like
/// `move`/`remove` that don't care which kind they're relocating.
#[derive(Debug, Clone)]
pub enum Entry {
    Plain(String),
    Github(GithubSource),
}

impl Entry {
    pub fn identifier(&self) -> &str {
        match self {
            Entry::Plain(s) => s,
            Entry::Github(g) => &g.name,
        }
    }
}

/// One category file, fully parsed: its name, path, and the plain nixpkgs
/// packages + GitHub-sourced packages it currently declares.
#[derive(Debug, Clone)]
pub struct Category {
    pub name: String,
    pub path: PathBuf,
    pub packages: Vec<String>,
    pub github: Vec<GithubSource>,
    raw: String,
}

impl Category {
    /// Build the canonical template for a brand-new, empty category file.
    /// (No `ghPkgs` block yet — that only appears once a GitHub source is added.)
    pub fn template(name: &str) -> String {
        format!(
            "# This file is managed by nixlayer.\n\
             # Category: {name}\n\
             # Do not hand-edit the package list unless you know what you're doing —\n\
             # nixlayer may reorder or rewrite it when you run `nixlayer add/remove/move`.\n\
             # Anything other than the environment.systemPackages list below is left alone.\n\
             \n\
             {{ pkgs, ... }}:\n\
             \n\
             {{\n\
             \x20 environment.systemPackages = with pkgs; [\n\
             \x20 ];\n\
             }}\n"
        )
    }

    /// Load a category from disk. Returns Err(UnparseableCategoryFile) if
    /// nixlayer doesn't recognize the shape of the package list or the
    /// GitHub-sources block — it refuses to guess in that case.
    pub fn load(name: &str, path: PathBuf) -> Result<Category> {
        let raw = fs::read_to_string(&path)?;
        let github = Self::parse_github(&path, &raw)?;
        let packages = Self::parse_packages(&path, &raw)?;
        Ok(Category {
            name: name.to_string(),
            path,
            packages,
            github,
            raw,
        })
    }

    /// Create an empty in-memory category backed by a not-yet-written file.
    pub fn new_empty(name: &str, path: PathBuf) -> Category {
        let raw = Self::template(name);
        Category {
            name: name.to_string(),
            path,
            packages: Vec::new(),
            github: Vec::new(),
            raw,
        }
    }

    fn parse_packages(path: &PathBuf, raw: &str) -> Result<Vec<String>> {
        let marker_idx = raw.find(MARKER).ok_or_else(|| {
            NixlayerError::UnparseableCategoryFile(
                path.clone(),
                "no environment.systemPackages found".into(),
            )
        })?;
        let block = find_bracket_block(raw, marker_idx).ok_or_else(|| {
            NixlayerError::UnparseableCategoryFile(path.clone(), "no [ ... ] list found".into())
        })?;
        tokenize_package_list(&block.inner)
            .map_err(|e| NixlayerError::UnparseableCategoryFile(path.clone(), format!("{e}")))
    }

    /// Parse the optional `ghPkgs = { ... };` block. Returns an empty Vec if
    /// there's no such block at all (the common case — most categories are
    /// plain nixpkgs packages). If the block exists but doesn't match
    /// nixlayer's exact generated shape, that's treated as unparseable rather
    /// than guessed at.
    fn parse_github(path: &PathBuf, raw: &str) -> Result<Vec<GithubSource>> {
        let Some(marker_idx) = raw.find(GH_MARKER) else {
            return Ok(Vec::new());
        };
        let brace_start = marker_idx + GH_MARKER.len() - 1; // land on the '{'
        let block = find_brace_block(raw, brace_start).ok_or_else(|| {
            NixlayerError::UnparseableCategoryFile(
                path.clone(),
                "ghPkgs = { ... } block is malformed".into(),
            )
        })?;

        // The systemPackages list must actually consume ghPkgs, or this file
        // has a stray/hand-written ghPkgs block nixlayer shouldn't manage.
        if !raw.contains(GH_SUFFIX) {
            return Err(NixlayerError::UnparseableCategoryFile(
                path.clone(),
                format!("found a ghPkgs block but no `{GH_SUFFIX}` in systemPackages"),
            ));
        }

        let mut out = Vec::new();
        let mut pending_ref: Option<String> = None;
        for raw_line in block.inner.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(caps) = SOURCE_REF_COMMENT_RE.captures(line) {
                pending_ref = Some(caps[1].to_string());
                continue;
            }
            let Some(caps) = GH_ENTRY_RE.captures(line) else {
                return Err(NixlayerError::UnparseableCategoryFile(
                    path.clone(),
                    format!("unrecognized line in ghPkgs block: '{line}'"),
                ));
            };
            let name = caps[1].to_string();
            let flake_ref = &caps[2];
            let attr = caps[3].to_string();
            let Some(ref_caps) = FLAKE_REF_RE.captures(flake_ref) else {
                return Err(NixlayerError::UnparseableCategoryFile(
                    path.clone(),
                    format!("'{flake_ref}' isn't a pinned github:owner/repo/<commit> reference"),
                ));
            };
            out.push(GithubSource {
                name,
                owner: ref_caps[1].to_string(),
                repo: ref_caps[2].to_string(),
                rev: ref_caps[3].to_string(),
                git_ref: pending_ref.take(),
                attr,
            });
        }
        Ok(out)
    }

    /// Render this category back to text. If there are no GitHub sources,
    /// this preserves the original raw text byte-for-byte outside the package
    /// list (same as always). Once a category has any GitHub sources,
    /// nixlayer regenerates the whole file from a template instead — it needs
    /// to keep the pinned-commit block in sync too, so the "preserve
    /// everything outside the list" guarantee only fully applies to plain
    /// categories.
    pub fn render(&self) -> Result<String> {
        // Once a file has EVER had a GitHub-sources block persisted to disk,
        // `raw` (loaded fresh each time) still contains it even if `self.github`
        // is now empty in memory — patching just the package list would leave
        // that stale block behind. So: only take the byte-preserving fast path
        // if there's no GitHub residue in `raw` at all; otherwise fully
        // regenerate (which correctly omits the block when github is empty).
        if self.github.is_empty() && !self.raw.contains(GH_MARKER) {
            self.render_simple()
        } else {
            Ok(self.render_regenerated())
        }
    }

    fn render_simple(&self) -> Result<String> {
        let marker_idx = self.raw.find(MARKER).ok_or_else(|| {
            NixlayerError::UnparseableCategoryFile(self.path.clone(), "marker missing".into())
        })?;
        let block = find_bracket_block(&self.raw, marker_idx).ok_or_else(|| {
            NixlayerError::UnparseableCategoryFile(self.path.clone(), "list missing".into())
        })?;

        let new_inner = render_package_list_inner(&self.packages);
        Ok(replace_bracket_inner(&self.raw, &block, &new_inner))
    }

    fn render_regenerated(&self) -> String {
        if self.github.is_empty() {
            // No GitHub sources left — regenerate the clean, plain shape
            // (same as a freshly-templated category), stripping any stale
            // ghPkgs block that used to be here.
            let pkg_lines = render_package_list_inner(&self.packages);
            return format!(
                "# This file is managed by nixlayer.\n\
                 # Category: {name}\n\
                 # Do not hand-edit the package list unless you know what you're doing —\n\
                 # nixlayer may reorder or rewrite it when you run `nixlayer add/remove/move`.\n\
                 # Anything other than the environment.systemPackages list below is left alone.\n\
                 \n\
                 {{ pkgs, ... }}:\n\
                 \n\
                 {{\n\
                 \x20 environment.systemPackages = with pkgs; [{pkg_lines}];\n\
                 }}\n",
                name = self.name,
            );
        }

        let mut sorted_gh = self.github.clone();
        sorted_gh.sort_by(|a, b| a.name.cmp(&b.name));

        let mut gh_block = String::new();
        for g in &sorted_gh {
            if let Some(r) = &g.git_ref {
                gh_block.push_str(&format!("    # nixlayer-source-ref: {r}\n"));
            }
            gh_block.push_str(&format!(
                "    {} = (builtins.getFlake \"{}\").packages.${{pkgs.system}}.{};\n",
                g.name,
                g.flake_ref(),
                g.attr
            ));
        }

        let mut sorted_pkgs = self.packages.clone();
        sorted_pkgs.sort();
        sorted_pkgs.dedup();
        let pkg_lines = if sorted_pkgs.is_empty() {
            String::new()
        } else {
            let mut s = String::new();
            for p in &sorted_pkgs {
                s.push_str("    ");
                s.push_str(p);
                s.push('\n');
            }
            s
        };

        format!(
            "# This file is managed by nixlayer.\n\
             # Category: {name}\n\
             # This category includes GitHub-sourced packages, pinned to exact commits.\n\
             # nixlayer manages this whole file once GitHub sources are present —\n\
             # avoid hand-editing it; use `nixlayer github` / `nixlayer add|remove|move` instead.\n\
             \n\
             {{ pkgs, ... }}:\n\
             let\n\
             \x20 ghPkgs = {{\n\
             {gh_block}\
             \x20 }};\n\
             in\n\
             {{\n\
             \x20 environment.systemPackages = with pkgs; [\n\
             {pkg_lines}\
             \x20 ]{GH_SUFFIX};\n\
             }}\n",
            name = self.name,
        )
    }

    pub fn contains(&self, package: &str) -> bool {
        self.packages.iter().any(|p| p == package)
    }

    pub fn contains_github(&self, name: &str) -> bool {
        self.github.iter().any(|g| g.name == name)
    }

    /// True if `id` matches either a plain package or a GitHub source name in this category.
    pub fn contains_any(&self, id: &str) -> bool {
        self.contains(id) || self.contains_github(id)
    }

    /// Add a plain nixpkgs package, returns false (no-op) if it was already present.
    pub fn add(&mut self, package: &str) -> bool {
        if self.contains_any(package) {
            return false;
        }
        self.packages.push(package.to_string());
        self.packages.sort();
        true
    }

    /// Remove a plain nixpkgs package, returns false if it wasn't present.
    pub fn remove(&mut self, package: &str) -> bool {
        let before = self.packages.len();
        self.packages.retain(|p| p != package);
        self.packages.len() != before
    }

    /// Add a GitHub-sourced package, returns false (no-op) if the name collides
    /// with an existing plain package or GitHub source in this category.
    pub fn add_github(&mut self, source: GithubSource) -> bool {
        if self.contains_any(&source.name) {
            return false;
        }
        self.github.push(source);
        true
    }

    pub fn remove_github(&mut self, name: &str) -> bool {
        let before = self.github.len();
        self.github.retain(|g| g.name != name);
        self.github.len() != before
    }

    /// Remove and return whichever entry (plain or GitHub) matches `id`, for
    /// generic operations like `move` that relocate an entry without needing
    /// to know its type up front.
    pub fn take_entry(&mut self, id: &str) -> Option<Entry> {
        if self.contains(id) {
            self.remove(id);
            return Some(Entry::Plain(id.to_string()));
        }
        if let Some(pos) = self.github.iter().position(|g| g.name == id) {
            return Some(Entry::Github(self.github.remove(pos)));
        }
        None
    }

    /// Insert a previously-`take_entry`'d entry into this category. Returns
    /// false (and does not insert) on a name collision.
    pub fn insert_entry(&mut self, entry: Entry) -> bool {
        match entry {
            Entry::Plain(p) => self.add(&p),
            Entry::Github(g) => self.add_github(g),
        }
    }

    /// All identifiers this category declares — plain package names and
    /// GitHub source names combined — for cross-category lookups like
    /// duplicate detection, where both kinds share the same namespace.
    pub fn all_identifiers(&self) -> Vec<String> {
        let mut v: Vec<String> = self.packages.clone();
        v.extend(self.github.iter().map(|g| g.name.clone()));
        v
    }

    pub fn write(&self) -> Result<()> {
        let rendered = self.render()?;
        fs::write(&self.path, rendered)?;
        Ok(())
    }
}

fn render_package_list_inner(packages: &[String]) -> String {
    let mut sorted = packages.to_vec();
    sorted.sort();
    sorted.dedup();

    if sorted.is_empty() {
        "\n  ".to_string()
    } else {
        let mut s = String::from("\n");
        for pkg in &sorted {
            s.push_str("    ");
            s.push_str(pkg);
            s.push('\n');
        }
        s.push_str("  ");
        s
    }
}

/// Load every category file in the nixlayer dir. Category files that fail to
/// parse are still returned (as Err entries keyed by name) so callers like
/// `doctor` can report them without crashing the whole command.
pub fn load_all(paths: &Paths) -> Result<Vec<(String, Result<Category>)>> {
    let names = paths.list_categories()?;
    let mut out = Vec::new();
    for name in names {
        let path = paths.category_file(&name);
        out.push((name.clone(), Category::load(&name, path)));
    }
    Ok(out)
}

/// Find which categories (there should be exactly zero or one in a healthy
/// setup) currently declare `identifier` — checking both plain packages and
/// GitHub source names, since they share one namespace.
pub fn find_package(paths: &Paths, identifier: &str) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for (name, cat) in load_all(paths)? {
        if let Ok(cat) = cat {
            if cat.contains_any(identifier) {
                found.push(name);
            }
        }
    }
    Ok(found)
}

/// Every duplicate: identifiers (plain or GitHub) declared in 2+ categories,
/// mapped to the list of categories they appear in. Empty means no duplicates.
pub fn find_duplicates(paths: &Paths) -> Result<Vec<(String, Vec<String>)>> {
    use std::collections::BTreeMap;
    let mut by_pkg: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, cat) in load_all(paths)? {
        if let Ok(cat) = cat {
            for pkg in cat.all_identifiers() {
                by_pkg.entry(pkg).or_default().push(name.clone());
            }
        }
    }
    Ok(by_pkg
        .into_iter()
        .filter(|(_, cats)| cats.len() > 1)
        .collect())
}

pub fn ensure_valid_category_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && name.chars().next().unwrap().is_ascii_alphabetic();
    if ok {
        Ok(())
    } else {
        Err(NixlayerError::Other(format!(
            "'{name}' isn't a valid category name (use letters, digits, - or _, starting with a letter)"
        )))
    }
}

/// Validate a local GitHub-source name — same rules as a category name
/// (identifier-safe, since it's used as `ghPkgs.<name>` directly in Nix).
pub fn ensure_valid_source_name(name: &str) -> Result<()> {
    ensure_valid_category_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn empty_template_roundtrips() {
        let cat = Category::new_empty("app", PathBuf::from("/tmp/app.nix"));
        let rendered = cat.render().unwrap();
        let reparsed = Category::parse_packages(&PathBuf::from("x"), &rendered).unwrap();
        assert!(reparsed.is_empty());
    }

    #[test]
    fn add_and_render_sorted() {
        let mut cat = Category::new_empty("app", PathBuf::from("/tmp/app.nix"));
        assert!(cat.add("vlc"));
        assert!(cat.add("firefox"));
        assert!(!cat.add("firefox")); // dup add is a no-op
        let rendered = cat.render().unwrap();
        assert!(rendered.contains("firefox"));
        assert!(rendered.contains("vlc"));
        let fx = rendered.find("firefox").unwrap();
        let vlc = rendered.find("vlc").unwrap();
        assert!(fx < vlc, "expected alphabetical order in output");
    }

    #[test]
    fn remove_package() {
        let mut cat = Category::new_empty("app", PathBuf::from("/tmp/app.nix"));
        cat.add("firefox");
        assert!(cat.remove("firefox"));
        assert!(!cat.remove("firefox"));
        assert!(cat.packages.is_empty());
    }

    #[test]
    fn preserves_header_comments_on_render() {
        let cat = Category::new_empty("gaming", PathBuf::from("/tmp/gaming.nix"));
        let rendered = cat.render().unwrap();
        assert!(rendered.starts_with("# This file is managed by nixlayer."));
        assert!(rendered.contains("Category: gaming"));
    }

    #[test]
    fn load_from_real_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gaming.nix");
        std::fs::write(&path, Category::template("gaming")).unwrap();
        let mut cat = Category::load("gaming", path.clone()).unwrap();
        cat.add("steam");
        cat.write().unwrap();
        let cat2 = Category::load("gaming", path).unwrap();
        assert_eq!(cat2.packages, vec!["steam".to_string()]);
    }

    #[test]
    fn unparseable_file_is_reported_not_panicked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weird.nix");
        std::fs::write(
            &path,
            "{ pkgs, ... }:\n{\n  environment.systemPackages = with pkgs; [\n    firefox\n    (python3.withPackages (p: [ p.numpy ]))\n  ];\n}\n",
        )
        .unwrap();
        let err = Category::load("weird", path).unwrap_err();
        assert!(matches!(err, NixlayerError::UnparseableCategoryFile(_, _)));
    }

    #[test]
    fn category_name_validation() {
        assert!(ensure_valid_category_name("gaming").is_ok());
        assert!(ensure_valid_category_name("dev-tools").is_ok());
        assert!(ensure_valid_category_name("2fast").is_err());
        assert!(ensure_valid_category_name("").is_err());
        assert!(ensure_valid_category_name("has space").is_err());
    }

    fn sample_source(name: &str) -> GithubSource {
        GithubSource {
            name: name.to_string(),
            owner: "hyprwm".to_string(),
            repo: "Hyprland".to_string(),
            rev: "8f3a91c2b1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8".to_string(),
            git_ref: Some("main".to_string()),
            attr: "default".to_string(),
        }
    }

    #[test]
    fn github_source_roundtrips_through_render_and_parse() {
        let mut cat = Category::new_empty("gaming", PathBuf::from("/tmp/gaming.nix"));
        cat.add("steam");
        assert!(cat.add_github(sample_source("hyprland-git")));
        let rendered = cat.render().unwrap();

        assert!(rendered.contains("ghPkgs = {"));
        assert!(rendered.contains("hyprland-git ="));
        assert!(rendered.contains("github:hyprwm/Hyprland/8f3a91c2b1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8"));
        assert!(rendered.contains("nixlayer-source-ref: main"));
        assert!(rendered.contains(GH_SUFFIX));
        assert!(rendered.contains("steam"));

        // Reparse from scratch and confirm both entries survive.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gaming.nix");
        std::fs::write(&path, &rendered).unwrap();
        let reparsed = Category::load("gaming", path).unwrap();
        assert_eq!(reparsed.packages, vec!["steam".to_string()]);
        assert_eq!(reparsed.github.len(), 1);
        assert_eq!(reparsed.github[0].name, "hyprland-git");
        assert_eq!(reparsed.github[0].owner, "hyprwm");
        assert_eq!(reparsed.github[0].rev, "8f3a91c2b1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8");
        assert_eq!(reparsed.github[0].git_ref.as_deref(), Some("main"));
    }

    #[test]
    fn category_without_github_stays_in_simple_format() {
        let mut cat = Category::new_empty("app", PathBuf::from("/tmp/app.nix"));
        cat.add("firefox");
        let rendered = cat.render().unwrap();
        assert!(!rendered.contains("ghPkgs"));
        assert!(!rendered.contains(GH_SUFFIX));
    }

    #[test]
    fn removing_last_github_source_falls_back_to_simple_format() {
        let mut cat = Category::new_empty("gaming", PathBuf::from("/tmp/gaming.nix"));
        cat.add("steam");
        cat.add_github(sample_source("hyprland-git"));
        assert!(cat.remove_github("hyprland-git"));
        let rendered = cat.render().unwrap();
        assert!(!rendered.contains("ghPkgs"), "should drop back to simple format once no github sources remain");
        assert!(rendered.contains("steam"));
    }

    #[test]
    fn removing_last_github_source_survives_a_real_disk_roundtrip() {
        // Regression test: the in-memory-only version of this test above
        // doesn't catch it, because `raw` there was never actually replaced
        // with real ghPkgs-block text. This one writes to disk, reloads (so
        // `raw` genuinely contains the ghPkgs block), removes, and writes
        // again — which used to leave the stale block behind and made the
        // "removed" entry reappear on the next load.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gaming.nix");

        let mut cat = Category::new_empty("gaming", path.clone());
        cat.add("steam");
        cat.add_github(sample_source("hyprland-git"));
        cat.write().unwrap();

        let mut reloaded = Category::load("gaming", path.clone()).unwrap();
        assert_eq!(reloaded.github.len(), 1);
        assert!(reloaded.remove_github("hyprland-git"));
        reloaded.write().unwrap();

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("ghPkgs"), "stale github block must not survive a real write");
        assert!(!on_disk.contains("hyprland-git"));

        let final_load = Category::load("gaming", path).unwrap();
        assert!(
            final_load.github.is_empty(),
            "removed entry must not reappear after reload"
        );
        assert_eq!(final_load.packages, vec!["steam".to_string()]);
    }

    #[test]
    fn take_entry_and_insert_entry_move_a_github_source() {
        let mut src_cat = Category::new_empty("app", PathBuf::from("/tmp/app.nix"));
        src_cat.add_github(sample_source("hyprland-git"));
        let entry = src_cat.take_entry("hyprland-git").unwrap();
        assert!(src_cat.github.is_empty());

        let mut dst_cat = Category::new_empty("gaming", PathBuf::from("/tmp/gaming.nix"));
        assert!(dst_cat.insert_entry(entry));
        assert_eq!(dst_cat.github.len(), 1);
        assert_eq!(dst_cat.github[0].name, "hyprland-git");
    }

    #[test]
    fn take_entry_handles_plain_packages_too() {
        let mut cat = Category::new_empty("app", PathBuf::from("/tmp/app.nix"));
        cat.add("firefox");
        let entry = cat.take_entry("firefox").unwrap();
        assert!(matches!(entry, Entry::Plain(s) if s == "firefox"));
        assert!(cat.packages.is_empty());
    }

    #[test]
    fn rejects_hand_edited_github_block_without_suffix() {
        // Someone manually added a ghPkgs block but didn't wire it into the
        // package list the way nixlayer generates — must refuse, not guess.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weird.nix");
        std::fs::write(
            &path,
            "{ pkgs, ... }:\nlet\n  ghPkgs = {\n    foo = (builtins.getFlake \"github:a/b/1234567\").packages.${pkgs.system}.default;\n  };\nin\n{\n  environment.systemPackages = with pkgs; [\n    firefox\n  ];\n}\n",
        )
        .unwrap();
        let err = Category::load("weird", path).unwrap_err();
        assert!(matches!(err, NixlayerError::UnparseableCategoryFile(_, _)));
    }

    #[test]
    fn duplicate_detection_spans_plain_and_github_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().to_path_buf());
        std::fs::create_dir_all(&paths.nixlayer_dir).unwrap();

        let mut a = Category::new_empty("app", paths.category_file("app"));
        a.add_github(sample_source("hyprland-git"));
        a.write().unwrap();

        let mut b = Category::new_empty("gaming", paths.category_file("gaming"));
        b.add("hyprland-git"); // same identifier, but as a plain "package" — should still count as a dup
        b.write().unwrap();

        let dups = find_duplicates(&paths).unwrap();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].0, "hyprland-git");
    }

    fn _use(_: &Path) {}
}
