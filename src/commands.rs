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
    if let Ok(p) = resolver::resolve_attribute(query) {
        return Ok(p);
    }

    let candidates = resolver::search(query)?;
    if candidates.is_empty() {
        return Err(NixlayerError::ResolverFailed(format!(
            "no nixpkgs package matches '{query}'"
        )));
    }

    let exact: Vec<&PackageInfo> = candidates
        .iter()
        .filter(|p| p.attribute == query || p.pname.as_deref() == Some(query))
        .collect();
    if exact.len() == 1 {
        return resolver::resolve_attribute(&exact[0].attribute);
    }

    // Prefer close fuzzy matches (either direction, so "spicetify" matches
    // "spicetify-cli" and a query of "gimp" would match "gimp-with-plugins").
    let close: Vec<&PackageInfo> = candidates
        .iter()
        .filter(|p| {
            p.attribute.contains(query)
                || query.contains(&p.attribute)
                || p.pname
                    .as_deref()
                    .map(|pn| pn.contains(query) || query.contains(pn))
                    .unwrap_or(false)
        })
        .collect();
    let pool: Vec<&PackageInfo> = if !close.is_empty() {
        close
    } else {
        candidates.iter().collect()
    };

    if pool.len() == 1 {
        let only = pool[0];
        let prompt = format!(
            "'{}' isn't an exact match — did you mean '{}'? ({})",
            query,
            only.attribute,
            only.description.as_deref().unwrap_or("")
        );
        if ui::confirm(&prompt) {
            return resolver::resolve_attribute(&only.attribute);
        }
        return Err(NixlayerError::ResolverFailed(
            "cancelled — re-run with the exact attribute name.".to_string(),
        ));
    }

    ui::heading(&format!(
        "'{query}' matched {} package(s):",
        pool.len().min(10)
    ));
    for (i, c) in pool.iter().take(10).enumerate() {
        println!(
            "  {}) {:<28} {}",
            i + 1,
            c.attribute,
            c.description.as_deref().unwrap_or("")
        );
    }
    match ui::prompt_choice(pool.len().min(10)) {
        Some(idx) => resolver::resolve_attribute(&pool[idx].attribute),
        None => Err(NixlayerError::ResolverFailed(
            "cancelled — re-run with the exact attribute name.".to_string(),
        )),
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

    if !resolved.free {
        let mut settings = crate::settings::Settings::load(&paths)?;
        if allow_unfree {
            // Flag path: don't ask, just do what was explicitly requested.
            if !settings.allow_unfree {
                settings.allow_unfree = true;
                settings.save(&paths)?;
                crate::settings::sync_default_nix(&paths, &settings)?;
            }
        } else {
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
                "NixOS will refuse to build unfree packages until allowUnfree is turned on."
            );
            let proceed = ui::confirm(
                "Enable allowUnfree now (writes nixpkgs.config.allowUnfree = true; into modules/nixlayer/default.nix) and add this package?",
            );
            if !proceed {
                println!(
                    "Left as-is. Re-run any time with --allow-unfree, or run `nixlayer allow-unfree` \
                     on its own whenever you're ready."
                );
                return Ok(());
            }
            if !settings.allow_unfree {
                settings.allow_unfree = true;
                settings.save(&paths)?;
                crate::settings::sync_default_nix(&paths, &settings)?;
            }
        }
        ui::ok("allowUnfree is enabled in modules/nixlayer/default.nix.");
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
        let taken = cat.take_entry(package);
        if taken.is_none() {
            continue;
        }
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

    let mut taken_entry = None;
    for cat_name in &found {
        let path = paths.category_file(cat_name);
        let mut cat = Category::load(cat_name, path)?;
        if let Some(entry) = cat.take_entry(package) {
            // Keep whichever one we find (there should only be one in a
            // healthy setup; if it's a duplicate across categories, this
            // move also cleans that up by consolidating into one place).
            taken_entry = Some(entry);
        }
        cat.write()?;
    }
    let Some(entry) = taken_entry else {
        return Err(NixlayerError::Other(format!(
            "'{package}' was reported as managed but couldn't be found when moving — this shouldn't happen, please report it."
        )));
    };

    let target_path = paths.category_file(target_category);
    let mut target = if target_path.is_file() {
        Category::load(target_category, target_path)?
    } else {
        Category::new_empty(target_category, target_path)
    };
    if !target.insert_entry(entry) {
        return Err(NixlayerError::PackageAlreadyExists(
            package.to_string(),
            target_category.to_string(),
        ));
    }
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
    let total = cat.packages.len() + cat.github.len();
    ui::heading(&format!("{} ({})", cat.name, total));
    if total == 0 {
        println!("  {}", ui::dim("(empty)"));
    }
    for pkg in &cat.packages {
        println!("  {pkg}");
    }
    for g in &cat.github {
        println!(
            "  {} {}",
            ui::dim("[github]"),
            format!(
                "{} -> {}/{}@{} ({})",
                g.name,
                g.owner,
                g.repo,
                &g.rev[..g.rev.len().min(10)],
                g.attr
            )
        );
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
        let count = result
            .as_ref()
            .map(|c| c.packages.len() + c.github.len())
            .unwrap_or(0);
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

pub fn allow_unfree(disable: bool) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let mut settings = crate::settings::Settings::load(&paths)?;
    settings.allow_unfree = !disable;
    settings.save(&paths)?;
    crate::settings::sync_default_nix(&paths, &settings)?;

    if disable {
        ui::ok("disabled allowUnfree in modules/nixlayer/default.nix");
    } else {
        ui::ok("enabled allowUnfree in modules/nixlayer/default.nix (nixpkgs.config.allowUnfree = true;)");
    }
    println!("{}", ui::dim("Run `nixlayer rebuild` to apply."));
    Ok(())
}

// ---------------------------------------------------------------------------
// GitHub-sourced packages
// ---------------------------------------------------------------------------

pub fn github_add(
    owner_repo: &str,
    git_ref: Option<&str>,
    attr: Option<&str>,
    name: Option<&str>,
    category_name: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let (owner, repo) = owner_repo.split_once('/').ok_or_else(|| {
        NixlayerError::Other(format!(
            "'{owner_repo}' should look like owner/repo, e.g. hyprwm/Hyprland"
        ))
    })?;

    let local_name = name.unwrap_or(repo).to_string();
    category::ensure_valid_source_name(&local_name)?;

    let existing = category::find_package(&paths, &local_name)?;
    if let Some(existing_cat) = existing.first() {
        return Err(NixlayerError::PackageAlreadyExists(
            local_name,
            existing_cat.clone(),
        ));
    }

    ui::heading(&format!("resolving {owner}/{repo} via nix flake metadata..."));
    let resolved = resolver::resolve_github(owner, repo, git_ref, attr)?;

    let source = category::GithubSource {
        name: local_name.clone(),
        owner: resolved.owner,
        repo: resolved.repo,
        rev: resolved.rev.clone(),
        git_ref: resolved.git_ref,
        attr: resolved.attr,
    };

    ui::ok(&format!(
        "pinned to commit {} ({})",
        &resolved.rev[..resolved.rev.len().min(12)],
        source.flake_ref()
    ));
    if let Some(d) = &resolved.description {
        println!("  description: {d}");
    }
    if let Some(h) = &resolved.homepage {
        println!("  homepage: {h}");
    }

    let category_name = category_name.unwrap_or(category::DEFAULT_CATEGORY);
    category::ensure_valid_category_name(category_name)?;
    let cat_path = paths.category_file(category_name);
    let mut cat = if cat_path.is_file() {
        Category::load(category_name, cat_path)?
    } else {
        Category::new_empty(category_name, cat_path)
    };
    cat.add_github(source);

    if dry_run {
        println!(
            "{} would add '{local_name}' to {}",
            ui::dim("[dry-run]"),
            cat.path.display()
        );
        print_file_preview(&cat)?;
        return Ok(());
    }

    cat.write()?;
    ui::ok(&format!(
        "added '{local_name}' (from github:{owner}/{repo}) to {}",
        cat.path.display()
    ));
    println!("{}", ui::dim("Run `nixlayer rebuild` to apply."));
    Ok(())
}

pub fn github_list() -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let mut any = false;
    for (cat_name, result) in category::load_all(&paths)? {
        let Ok(cat) = result else { continue };
        for g in &cat.github {
            any = true;
            let ref_hint = g.git_ref.as_deref().unwrap_or("(unpinned ref)");
            println!(
                "  {:<20} {}/{}@{} [{}] -> {} ({})",
                ui::bold(&g.name),
                g.owner,
                g.repo,
                &g.rev[..g.rev.len().min(10)],
                ref_hint,
                g.attr,
                cat_name
            );
        }
    }
    if !any {
        println!("No GitHub-sourced packages yet. `nixlayer github add <owner>/<repo>` to add one.");
    }
    Ok(())
}

pub fn github_remove(name: &str, dry_run: bool) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let found = category::find_package(&paths, name)?;
    if found.is_empty() {
        return Err(NixlayerError::PackageNotManaged(name.to_string()));
    }
    for cat_name in &found {
        let path = paths.category_file(cat_name);
        let mut cat = Category::load(cat_name, path)?;
        if !cat.remove_github(name) {
            ui::warn(&format!(
                "'{name}' is declared in {cat_name} but not as a GitHub source — skipping (use `nixlayer remove` for plain packages)."
            ));
            continue;
        }
        if dry_run {
            println!(
                "{} would remove '{name}' from {}",
                ui::dim("[dry-run]"),
                cat.path.display()
            );
        } else {
            cat.write()?;
            ui::ok(&format!("removed '{name}' from {}", cat.path.display()));
        }
    }
    if !dry_run {
        println!("{}", ui::dim("Run `nixlayer rebuild` to apply."));
    }
    Ok(())
}

pub fn github_update(name: &str, dry_run: bool) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let found = category::find_package(&paths, name)?;
    let Some(cat_name) = found.first() else {
        return Err(NixlayerError::PackageNotManaged(name.to_string()));
    };
    let path = paths.category_file(cat_name);
    let mut cat = Category::load(cat_name, path)?;
    let Some(existing) = cat.github.iter().find(|g| g.name == name).cloned() else {
        return Err(NixlayerError::Other(format!(
            "'{name}' is in {cat_name} but isn't a GitHub source."
        )));
    };

    ui::heading(&format!(
        "re-resolving {}/{}@{}...",
        existing.owner,
        existing.repo,
        existing.git_ref.as_deref().unwrap_or("(default branch)")
    ));
    let resolved = resolver::resolve_github(
        &existing.owner,
        &existing.repo,
        existing.git_ref.as_deref(),
        Some(&existing.attr),
    )?;

    if resolved.rev == existing.rev {
        println!("'{name}' is already pinned to the latest commit ({}).", &existing.rev[..existing.rev.len().min(12)]);
        return Ok(());
    }

    if dry_run {
        println!(
            "{} would move '{name}' from {} to {}",
            ui::dim("[dry-run]"),
            &existing.rev[..existing.rev.len().min(12)],
            &resolved.rev[..resolved.rev.len().min(12)]
        );
        return Ok(());
    }

    cat.remove_github(name);
    cat.add_github(category::GithubSource {
        name: name.to_string(),
        owner: resolved.owner,
        repo: resolved.repo,
        rev: resolved.rev.clone(),
        git_ref: resolved.git_ref,
        attr: resolved.attr,
    });
    cat.write()?;
    ui::ok(&format!(
        "updated '{name}': {} -> {}",
        &existing.rev[..existing.rev.len().min(12)],
        &resolved.rev[..resolved.rev.len().min(12)]
    ));
    println!("{}", ui::dim("Run `nixlayer rebuild` to apply."));
    Ok(())
}

// ---------------------------------------------------------------------------
// Import from nix-env / nix profile
// ---------------------------------------------------------------------------

pub fn import_profile(category_name: Option<&str>, dry_run: bool) -> Result<()> {
    let paths = Paths::discover()?;
    paths.require_initialized()?;

    let category_name = category_name.unwrap_or("import");
    category::ensure_valid_category_name(category_name)?;

    ui::heading("scanning nix-env / nix profile for imperatively-installed packages...");
    let entries = resolver::scan_profile()?;
    if entries.is_empty() {
        println!("Nothing found installed imperatively — nothing to import.");
        return Ok(());
    }

    let mut to_import = Vec::new();
    let mut skipped_managed = Vec::new();
    let mut unresolved = Vec::new();

    for entry in &entries {
        if !entry.likely_nixpkgs {
            unresolved.push((entry.clone(), "not clearly from nixpkgs — see `nixlayer github add` for non-nixpkgs sources".to_string()));
            continue;
        }
        let already = category::find_package(&paths, &entry.guessed_attr)?;
        if !already.is_empty() {
            skipped_managed.push((entry.clone(), already[0].clone()));
            continue;
        }
        match resolver::resolve_attribute(&entry.guessed_attr) {
            Ok(resolved) => to_import.push((entry.clone(), resolved)),
            Err(e) => unresolved.push((entry.clone(), format!("{e}"))),
        }
    }

    if to_import.is_empty() {
        println!("No new packages to import.");
    } else {
        let cat_path = paths.category_file(category_name);
        let mut cat = if cat_path.is_file() {
            Category::load(category_name, cat_path)?
        } else {
            Category::new_empty(category_name, cat_path)
        };
        for (_, resolved) in &to_import {
            cat.add(&resolved.attribute);
        }

        if dry_run {
            println!(
                "{} would add {} package(s) to {}:",
                ui::dim("[dry-run]"),
                to_import.len(),
                cat.path.display()
            );
            for (entry, resolved) in &to_import {
                println!("    {} (from {})", resolved.attribute, entry.display_name);
            }
        } else {
            cat.write()?;
            ui::ok(&format!(
                "imported {} package(s) into {}",
                to_import.len(),
                cat.path.display()
            ));
            for (entry, resolved) in &to_import {
                println!("    {} (from {})", resolved.attribute, entry.display_name);
            }
        }
    }

    if !skipped_managed.is_empty() {
        ui::warn(&format!(
            "{} already managed elsewhere, skipped:",
            skipped_managed.len()
        ));
        for (entry, cat_name) in &skipped_managed {
            println!("    {} (already in '{cat_name}')", entry.display_name);
        }
    }

    if !unresolved.is_empty() {
        ui::warn(&format!(
            "{} couldn't be confidently matched, not imported:",
            unresolved.len()
        ));
        for (entry, reason) in &unresolved {
            println!("    {} — {reason}", entry.display_name);
        }
    }

    if dry_run || to_import.is_empty() {
        return Ok(());
    }

    println!("{}", ui::dim("Run `nixlayer rebuild` once you're happy with import.nix."));

    let should_remove = ui::confirm(&format!(
        "\nRemove these {} package(s) from your nix-env/profile now? (only do this after you've confirmed the rebuild works)",
        to_import.len()
    ));
    if !should_remove {
        println!("Left your profile untouched. Remove them yourself later if you'd like:");
        for (entry, _) in &to_import {
            match entry.backend {
                resolver::ProfileBackend::NixEnv => println!("    nix-env -e {}", entry.removal_key),
                resolver::ProfileBackend::NixProfile => println!("    nix profile remove {}", entry.removal_key),
            }
        }
        return Ok(());
    }

    for (entry, _) in &to_import {
        match resolver::remove_from_profile(entry) {
            Ok(()) => ui::ok(&format!("removed '{}' from your profile", entry.display_name)),
            Err(e) => ui::error(&format!(
                "couldn't remove '{}' automatically: {e}\n  Check `nix profile list` / `nix-env -q` and remove it yourself if needed.",
                entry.display_name
            )),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::GithubSource;

    fn setup() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("NIXLAYER_CONFIG_DIR", dir.path());
        let paths = Paths::at(dir.path().to_path_buf());
        std::fs::create_dir_all(&paths.nixlayer_dir).unwrap();
        std::fs::write(paths.default_nix(), crate::default_nix::render(false)).unwrap();
        (dir, paths)
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
    fn move_pkg_relocates_a_github_source_without_corrupting_it() {
        let (_dir, paths) = setup();
        let mut gaming = Category::new_empty("gaming", paths.category_file("gaming"));
        gaming.add("steam");
        gaming.add_github(sample_source("hyprland-git"));
        gaming.write().unwrap();

        move_pkg("hyprland-git", "app", false).unwrap();

        let gaming_after = Category::load("gaming", paths.category_file("gaming")).unwrap();
        assert!(
            gaming_after.github.is_empty(),
            "should be removed from the source category"
        );
        assert_eq!(gaming_after.packages, vec!["steam".to_string()]);

        let app_after = Category::load("app", paths.category_file("app")).unwrap();
        assert_eq!(
            app_after.github.len(),
            1,
            "must still be a GitHub source, not a corrupted plain entry"
        );
        assert_eq!(app_after.github[0].name, "hyprland-git");
        assert_eq!(app_after.github[0].rev, sample_source("hyprland-git").rev);
        assert!(
            app_after.packages.is_empty(),
            "must NOT have been added as a bare plain package"
        );
    }

    #[test]
    fn remove_deletes_a_github_source_cleanly() {
        let (_dir, paths) = setup();
        let mut gaming = Category::new_empty("gaming", paths.category_file("gaming"));
        gaming.add("steam");
        gaming.add_github(sample_source("hyprland-git"));
        gaming.write().unwrap();

        remove("hyprland-git", false).unwrap();

        let gaming_after = Category::load("gaming", paths.category_file("gaming")).unwrap();
        assert!(gaming_after.github.is_empty());
        assert_eq!(gaming_after.packages, vec!["steam".to_string()]);
    }

    #[test]
    fn move_pkg_still_works_for_plain_packages() {
        let (_dir, paths) = setup();
        let mut app = Category::new_empty("app", paths.category_file("app"));
        app.add("firefox");
        app.write().unwrap();

        move_pkg("firefox", "browsers", false).unwrap();

        let app_after = Category::load("app", paths.category_file("app")).unwrap();
        assert!(app_after.packages.is_empty());
        let browsers_after = Category::load("browsers", paths.category_file("browsers")).unwrap();
        assert_eq!(browsers_after.packages, vec!["firefox".to_string()]);
    }

    #[test]
    fn allow_unfree_command_toggles_default_nix() {
        let (_dir, paths) = setup();
        allow_unfree(false).unwrap();
        let rendered = std::fs::read_to_string(paths.default_nix()).unwrap();
        assert!(rendered.contains("allowUnfree = true;"));

        allow_unfree(true).unwrap();
        let rendered = std::fs::read_to_string(paths.default_nix()).unwrap();
        assert!(!rendered.contains("allowUnfree"));
    }
}
