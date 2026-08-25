use crate::category::{self, Category, DEFAULT_CATEGORY};
use crate::error::{NixlayerError, Result};
use crate::paths::Paths;
use crate::rebuild;
use crate::resolver::{self, PackageInfo};
use crate::state::{self, State};
use crate::ui;

pub fn init(dry_run: bool) -> Result<()> {
    let report = crate::init::run(dry_run)?;
    let prefix = if dry_run { "[dry-run] " } else { "" };

    if report.created_dir {
        ui::ok(&format!("{prefix}create modules/nixlayer/"));
    }
    if report.created_default_nix {
        ui::ok(&format!("{prefix}write modules/nixlayer/default.nix"));
    }
    if report.created_default_category {
        ui::ok(&format!(
            "{prefix}write modules/nixlayer/{DEFAULT_CATEGORY}.nix (empty default category)"
        ));
    }
    if report.configuration_nix_already_ok {
        ui::ok("configuration.nix already imports nixlayer — nothing to change.");
    } else if report.configuration_nix_patched {
        ui::ok(&format!(
            "{prefix}add `./modules/nixlayer/default.nix` to configuration.nix's imports"
        ));
    }
    if let Some(manual) = &report.manual_step_needed {
        ui::warn("one manual step is needed:");
        println!("\n{manual}");
    }

    if dry_run {
        println!("\n{}", ui::dim("Dry run — nothing was written. Re-run without --dry-run to apply."));
    } else if report.manual_step_needed.is_none() {
        println!(
            "\n{} nixlayer is ready. Try `nixlayer add firefox`, then `nixlayer rebuild`.",
            ui::bold("Done —")
        );
    }
    Ok(())
}

pub fn search(query: &str) -> Result<()> {
    let results = resolver::search(query)?;
    if results.is_empty() {
        println!("No nixpkgs packages matched '{query}'.");
        return Ok(());
    }
    ui::heading(&format!("{} match(es) for '{query}':\n", results.len()));
    for pkg in &results {
        print_search_row(pkg);
    }
    println!(
        "\n{}",
        ui::dim("Run `nixlayer info <attribute>` for full details, or `nixlayer add <attribute>` to add one.")
    );
    Ok(())
}

fn print_search_row(pkg: &PackageInfo) {
    let version = pkg.version.as_deref().unwrap_or("?");
    let desc = pkg.description.as_deref().unwrap_or("");
    println!("  {:<28} {:<12} {}", ui::bold(&pkg.attribute), version, desc);
}

pub fn info(package: &str) -> Result<()> {
    let pkg = resolve_or_suggest(package)?;
    ui::heading(&pkg.attribute);
    if let Some(pname) = &pkg.pname {
        println!("  package name : {pname}");
    }
    if let Some(v) = &pkg.version {
        println!("  version      : {v}");
    }
    if let Some(d) = &pkg.description {
        println!("  description  : {d}");
    }
    if let Some(h) = &pkg.homepage {
        println!("  homepage     : {h}");
    }
    if !pkg.license.is_empty() {
        println!("  license      : {}", pkg.license.join(", "));
    }
    println!(
        "  status       : {}",
        if pkg.free { "free" } else { "UNFREE" }
    );
    if !pkg.free {
        println!(
            "\n{}",
            ui::dim(
                "This package is unfree. NixOS will refuse to build it unless you allow unfree \
                 packages, e.g. `nixpkgs.config.allowUnfree = true;` (globally) or via \
                 `nixpkgs.config.allowUnfreePredicate` (per-package) in your own configuration.\n\
                 nixlayer will not change that setting for you — add it yourself, then \
                 `nixlayer add` with `--allow-unfree`."
            )
        );
    }
    if pkg.broken {
        ui::warn("this package is currently marked broken in nixpkgs.");
    }
    println!(
        "\n{}",
        ui::dim(match pkg.source {
            resolver::Backend::Flakes => "source: nix eval (flakes, nixpkgs registry)",
            resolver::Backend::Legacy => "source: nix-env (legacy/channel mode — reduced metadata)",
        })
    );
    Ok(())
}

/// Try resolving `query` as an exact attribute first (the common case: firefox,
/// steam, vscode are all real top-level attributes). If that fails, fall back
/// to search and only auto-pick when there's exactly one unambiguous match.
fn resolve_or_suggest(query: &str) -> Result<PackageInfo> {
    match resolver::resolve_attribute(query) {
        Ok(p) => Ok(p),
        Err(_) => {
            let candidates = resolver::search(query)?;
            let exact: Vec<&PackageInfo> = candidates
                .iter()
                .filter(|p| p.attribute == query || p.pname.as_deref() == Some(query))
                .collect();
            if exact.len() == 1 {
                return resolver::resolve_attribute(&exact[0].attribute);
            }
            if candidates.is_empty() {
                return Err(NixlayerError::ResolverFailed(format!(
                    "no nixpkgs package matches '{query}'"
                )));
            }
            let mut msg = format!(
                "'{query}' isn't an exact nixpkgs attribute, and matched {} candidates:\n",
                candidates.len()
            );
            for c in candidates.iter().take(10) {
                msg.push_str(&format!(
                    "    {} ({})\n",
                    c.attribute,
                    c.description.as_deref().unwrap_or("")
                ));
            }
            msg.push_str("  Re-run with the exact attribute name.");
            Err(NixlayerError::ResolverFailed(msg))
        }
    }
}

pub fn add(
    package: &str,
    category_name: Option<&str>,
    dry_run: bool,
    allow_unfree: bool,
    do_rebuild: bool,
) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let category_name = category_name.unwrap_or(DEFAULT_CATEGORY);
    category::ensure_valid_category_name(category_name)?;

    let resolved = resolve_or_suggest(package)?;

    if resolved.broken {
        ui::warn(&format!(
            "'{}' is currently marked BROKEN in nixpkgs — it may fail to build.",
            resolved.attribute
        ));
    }

    if !resolved.free && !allow_unfree {
        ui::error(&format!(
            "'{}' is unfree (license: {}).",
            resolved.attribute,
            if resolved.license.is_empty() {
                "proprietary".to_string()
            } else {
                resolved.license.join(", ")
            }
        ));
        println!(
            "\nNixOS will refuse to build unfree packages unless you allow them, e.g.:\n\n    \
             nixpkgs.config.allowUnfree = true;\n\n\
             in your own configuration (nixlayer will not add this for you). Once that's in \
             place, re-run with --allow-unfree to add it here."
        );
        return Ok(());
    }

    let existing = category::find_package(&paths, &resolved.attribute)?;
    if let Some(existing_cat) = existing.first() {
        return Err(NixlayerError::PackageAlreadyExists(
            resolved.attribute.clone(),
            existing_cat.clone(),
        ));
    }

    let cat_path = paths.category_file(category_name);
    let mut cat = if cat_path.is_file() {
        Category::load(category_name, cat_path)?
    } else {
        Category::new_empty(category_name, cat_path)
    };
    cat.add(&resolved.attribute);

    if dry_run {
        println!(
            "{} would add '{}' to {}",
            ui::dim("[dry-run]"),
            resolved.attribute,
            cat.path.display()
        );
        print_file_preview(&cat)?;
        return Ok(());
    }

    cat.write()?;
    ui::ok(&format!(
        "added '{}' to {}",
        resolved.attribute,
        cat.path.display()
    ));
    if !resolved.free {
        ui::warn("remember: allowUnfree must be enabled in your own config for this to build.");
    }
    println!("{}", ui::dim("Run `nixlayer rebuild` to apply."));

    if do_rebuild {
        rebuild::run(rebuild::Mode::Switch, false)?;
        ui::ok("rebuilt and activated.");
    }

    Ok(())
}

fn print_file_preview(cat: &Category) -> Result<()> {
    let rendered = cat.render()?;
    for line in rendered.lines() {
        println!("    {line}");
    }
    Ok(())
}

pub fn remove(package: &str, dry_run: bool) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let found = category::find_package(&paths, package)?;
    if found.is_empty() {
        return Err(NixlayerError::PackageNotManaged(package.to_string()));
    }

    for cat_name in &found {
        let path = paths.category_file(cat_name);
        let mut cat = Category::load(cat_name, path)?;
        cat.remove(package);
        if dry_run {
            println!(
                "{} would remove '{}' from {}",
                ui::dim("[dry-run]"),
                package,
                cat.path.display()
            );
        } else {
            cat.write()?;
            ui::ok(&format!("removed '{package}' from {}", cat.path.display()));
        }
    }
    if !dry_run {
        println!("{}", ui::dim("Run `nixlayer rebuild` to apply."));
    }
    Ok(())
}

pub fn move_pkg(package: &str, target_category: &str, dry_run: bool) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;
    category::ensure_valid_category_name(target_category)?;

    let found = category::find_package(&paths, package)?;
    if found.is_empty() {
        return Err(NixlayerError::PackageNotManaged(package.to_string()));
    }
    if found == vec![target_category.to_string()] {
        println!("'{package}' is already in '{target_category}' — nothing to do.");
        return Ok(());
    }

    if dry_run {
        for cat_name in &found {
            println!(
                "{} would remove '{}' from {}",
                ui::dim("[dry-run]"),
                package,
                paths.category_file(cat_name).display()
            );
        }
        println!(
            "{} would add '{}' to {}",
            ui::dim("[dry-run]"),
            package,
            paths.category_file(target_category).display()
        );
        return Ok(());
    }

    for cat_name in &found {
        let path = paths.category_file(cat_name);
        let mut cat = Category::load(cat_name, path)?;
        cat.remove(package);
        cat.write()?;
    }

    let target_path = paths.category_file(target_category);
    let mut target = if target_path.is_file() {
        Category::load(target_category, target_path)?
    } else {
        Category::new_empty(target_category, target_path)
    };
    target.add(package);
    target.write()?;

    ui::ok(&format!(
        "moved '{package}' from {} to {target_category}",
        found.join(", ")
    ));
    println!("{}", ui::dim("Run `nixlayer rebuild` to apply."));
    Ok(())
}

pub fn list(category_filter: Option<&str>) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    if let Some(name) = category_filter {
        let path = paths.category_file(name);
        if !path.is_file() {
            let known = paths.list_categories()?.join(", ");
            return Err(NixlayerError::UnknownCategory(name.to_string(), known));
        }
        let cat = Category::load(name, path)?;
        print_category(&cat);
        return Ok(());
    }

    let all = category::load_all(&paths)?;
    if all.is_empty() {
        println!("No categories yet. `nixlayer add <package>` to create one.");
        return Ok(());
    }
    let mut had_error = false;
    for (name, result) in &all {
        match result {
            Ok(cat) => print_category(cat),
            Err(e) => {
                ui::error(&format!("{name}: {e}"));
                had_error = true;
            }
        }
        println!();
    }
    if had_error {
        std::process::exit(1);
    }
    Ok(())
}

fn print_category(cat: &Category) {
    ui::heading(&format!("{} ({})", cat.name, cat.packages.len()));
    if cat.packages.is_empty() {
        println!("  {}", ui::dim("(empty)"));
    }
    for pkg in &cat.packages {
        println!("  {pkg}");
    }
}

pub fn categories() -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;
    let all = category::load_all(&paths)?;
    if all.is_empty() {
        println!("No categories yet.");
        return Ok(());
    }
    for (name, result) in &all {
        let count = result.as_ref().map(|c| c.packages.len()).unwrap_or(0);
        let status = if result.is_err() { " (parse error!)" } else { "" };
        println!("  {:<20} {} package(s){status}", name, count);
    }
    Ok(())
}

pub fn where_pkg(package: &str) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;
    let found = category::find_package(&paths, package)?;
    if found.is_empty() {
        println!("'{package}' is not managed by nixlayer.");
    } else if found.len() == 1 {
        println!(
            "'{package}' is in category '{}' ({})",
            found[0],
            paths.category_file(&found[0]).display()
        );
    } else {
        ui::warn(&format!(
            "'{package}' is declared in {} categories (this is a conflict): {}",
            found.len(),
            found.join(", ")
        ));
        println!("Resolve with: nixlayer move {package} <category>");
    }
    Ok(())
}

pub fn diff() -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let old = State::load(&paths)?;
    let new = State::capture_current(&paths)?;
    let changes = state::diff(&old, &new);

    if old.categories.is_empty() {
        println!(
            "{}",
            ui::dim("No rebuild recorded yet — the first `nixlayer rebuild` will apply everything below.")
        );
    }

    if changes.is_empty() {
        println!("No pending changes since the last rebuild.");
        return Ok(());
    }

    for c in &changes {
        ui::heading(&c.category);
        for pkg in &c.added {
            println!("  + {pkg}");
        }
        for pkg in &c.removed {
            println!("  - {pkg}");
        }
        println!();
    }
    Ok(())
}

pub fn rebuild(mode: rebuild::Mode, dry_run: bool) -> Result<()> {
    let paths = Paths::discover()?;

    rebuild::validate(&paths)?;
    ui::ok("all nixlayer-managed files are valid, no duplicates found.");

    if dry_run {
        println!("{}", ui::dim("[dry-run] validation passed; not invoking nixos-rebuild."));
        return Ok(());
    }

    println!("Running `nixos-rebuild {}`...", mode.as_arg());
    rebuild::run(mode, false)?;
    ui::ok("rebuild complete.");
    Ok(())
}

pub fn doctor() -> Result<()> {
    let report = crate::doctor::run();
    for f in &report.findings {
        match f.level {
            crate::doctor::Level::Ok => ui::ok(&f.message),
            crate::doctor::Level::Warn => ui::warn(&f.message),
            crate::doctor::Level::Error => ui::error(&f.message),
        }
    }
    if report.has_errors() {
        std::process::exit(1);
    }
    Ok(())
}
