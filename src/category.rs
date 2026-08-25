use std::fs;
use std::path::PathBuf;

use crate::error::{NixlayerError, Result};
use crate::nixfile::{find_bracket_block, replace_bracket_inner, tokenize_package_list};
use crate::paths::Paths;

pub const DEFAULT_CATEGORY: &str = "app";
const MARKER: &str = "environment.systemPackages";

/// One category file, fully parsed: its name, path, and the sorted, deduped
/// package list it currently declares.
#[derive(Debug, Clone)]
pub struct Category {
    pub name: String,
    pub path: PathBuf,
    pub packages: Vec<String>,
    raw: String,
}

impl Category {
    /// Build the canonical template for a brand-new, empty category file.
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

    /// Load a category from disk. Returns Err(UnparseableCategoryFile) if the
    /// package list contains anything nixlayer doesn't recognize as a plain
    /// attribute path — nixlayer refuses to guess in that case.
    pub fn load(name: &str, path: PathBuf) -> Result<Category> {
        let raw = fs::read_to_string(&path)?;
        let packages = Self::parse_packages(&path, &raw)?;
        Ok(Category {
            name: name.to_string(),
            path,
            packages,
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
            raw,
        }
    }

    fn parse_packages(path: &PathBuf, raw: &str) -> Result<Vec<String>> {
        let marker_idx = raw
            .find(MARKER)
            .ok_or_else(|| NixlayerError::UnparseableCategoryFile(path.clone(), "no environment.systemPackages found".into()))?;
        let block = find_bracket_block(raw, marker_idx).ok_or_else(|| {
            NixlayerError::UnparseableCategoryFile(path.clone(), "no [ ... ] list found".into())
        })?;
        tokenize_package_list(&block.inner)
            .map_err(|e| NixlayerError::UnparseableCategoryFile(path.clone(), format!("{e}")))
    }

    /// Render this category back to text with its current `packages`, sorted and
    /// deduplicated, substituted into the systemPackages list. All other text in
    /// the file (comments, header, braces) is preserved byte-for-byte.
    pub fn render(&self) -> Result<String> {
        let marker_idx = self.raw.find(MARKER).ok_or_else(|| {
            NixlayerError::UnparseableCategoryFile(self.path.clone(), "marker missing".into())
        })?;
        let block = find_bracket_block(&self.raw, marker_idx).ok_or_else(|| {
            NixlayerError::UnparseableCategoryFile(self.path.clone(), "list missing".into())
        })?;

        let mut sorted = self.packages.clone();
        sorted.sort();
        sorted.dedup();

        let new_inner = if sorted.is_empty() {
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
        };

        Ok(replace_bracket_inner(&self.raw, &block, &new_inner))
    }

    pub fn contains(&self, package: &str) -> bool {
        self.packages.iter().any(|p| p == package)
    }

    /// Add a package, returns false (no-op) if it was already present.
    pub fn add(&mut self, package: &str) -> bool {
        if self.contains(package) {
            return false;
        }
        self.packages.push(package.to_string());
        self.packages.sort();
        true
    }

    /// Remove a package, returns false if it wasn't present.
    pub fn remove(&mut self, package: &str) -> bool {
        let before = self.packages.len();
        self.packages.retain(|p| p != package);
        self.packages.len() != before
    }

    pub fn write(&self) -> Result<()> {
        let rendered = self.render()?;
        fs::write(&self.path, rendered)?;
        Ok(())
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
/// setup) currently declare `package`.
pub fn find_package(paths: &Paths, package: &str) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for (name, cat) in load_all(paths)? {
        if let Ok(cat) = cat {
            if cat.contains(package) {
                found.push(name);
            }
        }
    }
    Ok(found)
}

/// Every duplicate: packages declared in 2+ categories, mapped to the list of
/// categories they appear in. Empty means no duplicates.
pub fn find_duplicates(paths: &Paths) -> Result<Vec<(String, Vec<String>)>> {
    use std::collections::BTreeMap;
    let mut by_pkg: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, cat) in load_all(paths)? {
        if let Ok(cat) = cat {
            for pkg in cat.packages {
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

    fn _use(_: &Path) {}
}
