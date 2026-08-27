# Changelog

All notable changes to nixlayer will be documented in this file.

## [Unreleased]

## [v0.2.0]
Github support and fuzzy options

### Added
- `github` command to add apps that arent in nix packages 
  (theoretically could work with NUR but dedicated support will be added later)
- fuzzy search and do you mean when something goes wrong
- dedicated command for allowing unfree packages
- `import profile` this command allows you to import nix profiles and nix-env packages directly into the distro
  (optionally removes it from nix profile after import)

### Removed
Nothing

### Fixes
Nothing once again

### Install here
[0.2.0]: https://github.com/ssunflake/nixlayer/releases/tag/v0.2.0

## [v0.1.0]

### Added
- `init` command to easily setup nixlayer to your config
- `search` and `info` for nixpkg lookup with nix-env fallback
- `add`, `move` and `remove` to manage packages across categories
- `list`, `categories` and `where` to inspect currently managed packages
- `diff` to see what a rebuild would change
- `rebuild` - validates everything then rebuilds nixos
- `doctor` - checks avalibility, module wiring and duplicate packages
that was a handful.

### Removed
Nothing its the initial release

# Fixes
Nothing again

### Install here
[0.1.0]: https://github.com/ssunflake/nixlayer/releases/tag/v0.1.0
