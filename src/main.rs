use clap::Parser;
use nixlayer::cli::{Cli, Command};
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
    };

    if let Err(e) = result {
        ui::error(&format!("{e}"));
        std::process::exit(1);
    }
}
