use crate::category;
use crate::nixfile::{which, SyntaxCheck};
use crate::paths::Paths;
use crate::resolver::{self, Backend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Error,
}

#[derive(Debug)]
pub struct Finding {
    pub level: Level,
    pub message: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.level == Level::Error)
    }
}

pub fn run() -> Report {
    let mut findings = Vec::new();

    // 1. Nix availability
    match resolver::detect_backend() {
        Ok(Backend::Flakes) => findings.push(ok("`nix` found — using flake-based search/eval.")),
        Ok(Backend::Legacy) => findings.push(warn(
            "`nix` not found but `nix-env` is — running in legacy mode (reduced package metadata: no license/homepage). Install a modern `nix` for full search results.",
        )),
        Err(_) => findings.push(error(
            "no Nix package manager found at all (checked `nix`, `nix-env`). nixlayer needs Nix to search/validate packages.",
        )),
    }

    if which("nix-instantiate").is_none() && which("nix").is_none() {
        findings.push(warn(
            "no `nix-instantiate` or `nix` found — nixlayer cannot validate generated Nix syntax on this machine.",
        ));
    }

    // 2. Config discovery
    let paths = match Paths::discover() {
        Ok(p) => p,
        Err(e) => {
            findings.push(error(&format!("{e}")));
            return Report { findings };
        }
    };
    findings.push(ok(&format!(
        "NixOS configuration found at {}",
        paths.config_root.display()
    )));

    // 3. nixlayer initialized?
    if !paths.is_initialized() {
        findings.push(warn(&format!(
            "nixlayer is not initialized yet ({} missing). Run `nixlayer init`.",
            paths.nixlayer_dir.display()
        )));
        return Report { findings };
    }
    findings.push(ok(&format!(
        "nixlayer module present at {}",
        paths.nixlayer_dir.display()
    )));

    // 4. default.nix sanity
    if !paths.default_nix().is_file() {
        findings.push(error("modules/nixlayer/default.nix is missing."));
    } else if let SyntaxCheck::Invalid(msg) = crate::nixfile::validate_syntax(&paths.default_nix())
    {
        findings.push(error(&format!("default.nix has invalid Nix syntax: {msg}")));
    }

    // 5. Is nixlayer actually wired into configuration.nix?
    match &paths.configuration_nix {
        Some(cfg) => {
            let text = std::fs::read_to_string(cfg).unwrap_or_default();
            if text.contains("modules/nixlayer/default.nix") {
                findings.push(ok("configuration.nix imports the nixlayer module."));
            } else {
                findings.push(warn(
                    "configuration.nix does not appear to import modules/nixlayer/default.nix — nixlayer's packages won't be built. Run `nixlayer init` again or add the import by hand.",
                ));
            }
        }
        None => findings.push(warn(
            "no configuration.nix found at the config root — could not verify the nixlayer import. If you wire imports through flake.nix or another module, make sure modules/nixlayer/default.nix is imported somewhere.",
        )),
    }

    // 6. Category files: parse errors, syntax, empty, unfree
    let categories = category::load_all(&paths).unwrap_or_default();
    if categories.is_empty() {
        findings.push(warn("no category files exist yet — `nixlayer add <package>` to create one."));
    }
    for (name, result) in &categories {
        match result {
            Ok(cat) => {
                let path = paths.category_file(name);
                if let SyntaxCheck::Invalid(msg) = crate::nixfile::validate_syntax(&path) {
                    findings.push(error(&format!(
                        "{}: invalid Nix syntax: {msg}",
                        path.display()
                    )));
                }
                if cat.packages.is_empty() {
                    findings.push(warn(&format!("category '{name}' has no packages.")));
                }
            }
            Err(e) => {
                findings.push(error(&format!("category '{name}' could not be parsed: {e}")));
            }
        }
    }

    // 7. Duplicates across categories
    match category::find_duplicates(&paths) {
        Ok(dups) if !dups.is_empty() => {
            for (pkg, cats) in dups {
                findings.push(error(&format!(
                    "duplicate package declaration: '{pkg}' appears in: {}",
                    cats.join(", ")
                )));
            }
        }
        Ok(_) => findings.push(ok("no duplicate package declarations across categories.")),
        Err(e) => findings.push(error(&format!("could not check for duplicates: {e}"))),
    }

    // 8. Unfree packages: informational, best-effort (only if flakes backend, cheap eval per pkg
    //    would be slow for many packages, so we only warn generically here; `info`/`add` give
    //    the authoritative per-package unfree status).
    findings.push(Finding {
        level: Level::Ok,
        message:
            "Tip: `nixlayer info <package>` shows whether a specific package is unfree before you add it."
                .to_string(),
    });

    Report { findings }
}

fn ok(msg: &str) -> Finding {
    Finding {
        level: Level::Ok,
        message: msg.to_string(),
    }
}
fn warn(msg: &str) -> Finding {
    Finding {
        level: Level::Warn,
        message: msg.to_string(),
    }
}
fn error(msg: &str) -> Finding {
    Finding {
        level: Level::Error,
        message: msg.to_string(),
    }
}
