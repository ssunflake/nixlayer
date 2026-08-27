use clap::Parser;
use nixlayer::cli::{Cli, Command, GithubAction, ImportAction};
use nixlayer::{commands, ui};

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Init { dry_run } => commands::init(dry_run),
        Command::Search { query } => commands::search(&query),
        Command::Info { package } => commands::info(&package),
        Command::Add {
            package,
            category,
            dry_run,
            allow_unfree,
            rebuild,
        } => commands::add(&package, category.as_deref(), dry_run, allow_unfree, rebuild),
        Command::Remove { package, dry_run } => commands::remove(&package, dry_run),
        Command::Move {
            package,
            category,
            dry_run,
        } => commands::move_pkg(&package, &category, dry_run),
        Command::List { category } => commands::list(category.as_deref()),
        Command::Categories => commands::categories(),
        Command::Where { package } => commands::where_pkg(&package),
        Command::Diff => commands::diff(),
        Command::Rebuild { mode, dry_run } => commands::rebuild(mode.into(), dry_run),
        Command::Doctor => commands::doctor(),
        Command::AllowUnfree { disable } => commands::allow_unfree(disable),
        Command::Github { action } => match action {
            GithubAction::Add {
                owner_repo,
                r#ref,
                attr,
                name,
                category,
                dry_run,
            } => commands::github_add(
                &owner_repo,
                r#ref.as_deref(),
                attr.as_deref(),
                name.as_deref(),
                category.as_deref(),
                dry_run,
            ),
            GithubAction::List => commands::github_list(),
            GithubAction::Remove { name, dry_run } => commands::github_remove(&name, dry_run),
            GithubAction::Update { name, dry_run } => commands::github_update(&name, dry_run),
        },
        Command::Import { action } => match action {
            ImportAction::Profile { category, dry_run } => {
                commands::import_profile(category.as_deref(), dry_run)
            }
        },
    };

    if let Err(e) = result {
        ui::error(&format!("{e}"));
        std::process::exit(1);
    }
}

