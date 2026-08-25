//! nixlayer owns one extra small file: `.nixlayer-state.json`, a snapshot of the
//! package lists as of the last successful `nixlayer rebuild`. This is what lets
//! `nixlayer diff` show "what would change if you rebuilt now" without having to
//! interrogate the live system (`/run/current-system`), which would tie nixlayer
//! to internals that vary across NixOS versions.

use std::collections::BTreeMap;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::paths::Paths;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct State {
    /// category name -> sorted package list, as of the last successful rebuild.
    pub categories: BTreeMap<String, Vec<String>>,
}

impl State {
    pub fn load(paths: &Paths) -> Result<State> {
        let path = paths.state_file();
        if !path.is_file() {
            return Ok(State::default());
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let path = paths.state_file();
        let raw = serde_json::to_string_pretty(self)?;
        fs::write(path, raw)?;
        Ok(())
    }

    pub fn capture_current(paths: &Paths) -> Result<State> {
        let mut categories = BTreeMap::new();
        for (name, cat) in crate::category::load_all(paths)? {
            if let Ok(cat) = cat {
                let mut pkgs = cat.packages;
                pkgs.sort();
                categories.insert(name, pkgs);
            }
        }
        Ok(State { categories })
    }
}

/// Per-category added/removed packages between two states.
#[derive(Debug)]
pub struct CategoryDiff {
    pub category: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

pub fn diff(old: &State, new: &State) -> Vec<CategoryDiff> {
    let mut all_cats: Vec<&String> = old.categories.keys().chain(new.categories.keys()).collect();
    all_cats.sort();
    all_cats.dedup();

    let mut out = Vec::new();
    for cat in all_cats {
        let empty = Vec::new();
        let old_pkgs = old.categories.get(cat).unwrap_or(&empty);
        let new_pkgs = new.categories.get(cat).unwrap_or(&empty);

        let added: Vec<String> = new_pkgs
            .iter()
            .filter(|p| !old_pkgs.contains(p))
            .cloned()
            .collect();
        let removed: Vec<String> = old_pkgs
            .iter()
            .filter(|p| !new_pkgs.contains(p))
            .cloned()
            .collect();

        if !added.is_empty() || !removed.is_empty() {
            out.push(CategoryDiff {
                category: cat.clone(),
                added,
                removed,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(pairs: &[(&str, &[&str])]) -> State {
        let mut categories = BTreeMap::new();
        for (name, pkgs) in pairs {
            categories.insert(
                name.to_string(),
                pkgs.iter().map(|s| s.to_string()).collect(),
            );
        }
        State { categories }
    }

    #[test]
    fn detects_additions_and_removals() {
        let old = state(&[("app", &["firefox"]), ("gaming", &["steam"])]);
        let new = state(&[("app", &["firefox", "vlc"]), ("gaming", &[])]);
        let d = diff(&old, &new);
        assert_eq!(d.len(), 2);
        let app = d.iter().find(|c| c.category == "app").unwrap();
        assert_eq!(app.added, vec!["vlc".to_string()]);
        assert!(app.removed.is_empty());
        let gaming = d.iter().find(|c| c.category == "gaming").unwrap();
        assert_eq!(gaming.removed, vec!["steam".to_string()]);
    }

    #[test]
    fn no_diff_when_equal() {
        let a = state(&[("app", &["firefox"])]);
        let b = state(&[("app", &["firefox"])]);
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn new_category_shows_as_all_additions() {
        let old = state(&[]);
        let new = state(&[("browsers", &["firefox", "chromium"])]);
        let d = diff(&old, &new);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].category, "browsers");
        assert_eq!(d[0].added.len(), 2);
    }
}
