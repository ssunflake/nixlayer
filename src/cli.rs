use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "nixlayer",
    version,
    about = "A focused package-list manager for NixOS.\n\nnixlayer owns modules/nixlayer/ inside your NixOS configuration and nothing else. \nIt never touches hardware-configuration.nix, your bootloader, your window \nmanager config, or the rest of configuration.nix beyond one import line.",
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Set up modules/nixlayer/ and wire it into your NixOS configuration.
    Init {
        /// Show what would be created/changed without writing anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Search nixpkgs for packages matching a query.
    Search {
        query: String,
    },

    /// Show detailed info about a specific nixpkgs attribute.
    Info {
        package: String,
    },

    /// Add a package to a category (default: app).
    Add {
        /// Package name or exact nixpkgs attribute (e.g. firefox, nodePackages.typescript).
        package: String,
        #[arg(long, short)]
        category: Option<String>,
        /// Show what would change without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Add even if the package is unfree (still won't change allowUnfree for you).
        #[arg(long)]
        allow_unfree: bool,
        /// Rebuild immediately after adding (default is declarative-only).
        #[arg(long)]
        rebuild: bool,
    },

    /// Remove a package from whichever category currently declares it.
    Remove {
        package: String,
        #[arg(long)]
        dry_run: bool,
    },

    /// Move a package from its current category to another.
    Move {
        package: String,
        category: String,
        #[arg(long)]
        dry_run: bool,
    },

    /// List all managed packages, or just one category.
    List {
        category: Option<String>,
    },

    /// List all known categories.
    Categories,

    /// Show which category (if any) currently declares a package.
    Where {
        package: String,
    },

    /// Show what would change on the next `nixlayer rebuild`.
    Diff,

    /// Validate everything nixlayer manages and run nixos-rebuild.
    Rebuild {
        #[arg(value_enum, default_value_t = RebuildMode::Switch)]
        mode: RebuildMode,
        /// Validate only; don't actually invoke nixos-rebuild.
        #[arg(long)]
        dry_run: bool,
    },

    /// Diagnose the health of your nixlayer setup.
    Doctor,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
pub enum RebuildMode {
    Switch,
    Boot,
    Test,
}

impl From<RebuildMode> for crate::rebuild::Mode {
    fn from(m: RebuildMode) -> Self {
        match m {
            RebuildMode::Switch => crate::rebuild::Mode::Switch,
            RebuildMode::Boot => crate::rebuild::Mode::Boot,
            RebuildMode::Test => crate::rebuild::Mode::Test,
        }
    }
}
