# nixlayer

A focused package-list manager for NixOS

![tests](https://github.com/ssunflake/nixlayer/actions/workflows/build-and-cache.yml/badge.svg)
^ Doesnt work because i forgot to fix parralel testing!! ^
will fix


nixlayer manages **one small directory** inside your NixOS configuration
`modules/nixlayer/` — and nothing else. It is not a general NixOS config editor.
It doesn't touch your hardware config, bootloader, window manager, or the rest
of `configuration.nix` beyond a single import line it adds once, during `init`.

## Why

Editing `environment.systemPackages` by hand works fine until your
`configuration.nix` turns into a 400-line wall of unsorted package names with
no structure. nixlayer gives you a real "package manager" workflow —
search, add, remove, move between groups — while keeping the result as plain,
readable, hand-editable Nix. You can delete nixlayer entirely at any point and
keep every file it wrote; nothing about your system depends on nixlayer being
installed.

## Terminal Demo
-Insert later


## Install

Reproducibly, via the included flake:

```
nix build github:ssunflake/nixlayer          # or a local checkout: nix build .
./result/bin/nixlayer --version
```

or add it to your system packages:

```nix
{
  inputs.nixlayer.url = "github:ssunflake/nixlayer";

  outputs = { nixpkgs, nixlayer, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        nixlayer.nixosModules.default   # installs the `nixlayer` CLI only
        ./configuration.nix
      ];
    };
  };
}
```

Or just `cargo build --release` and drop the binary on your `PATH` — it has no
runtime dependency on being installed via Nix, only on `nix`/`nix-env` being
present on the machine it runs on (which is true of any NixOS system).

## Commands

| Command | Effect |
|---|---|
| `nixlayer init` | Create `modules/nixlayer/` and wire it into `configuration.nix` |
| `nixlayer search <query>` | Search nixpkgs |
| `nixlayer info <package>` | Full metadata for one package (license, unfree status, homepage...) |
| `nixlayer add <package> [--category X]` | Add to a category (default: `app`) |
| `nixlayer remove <package>` | Remove from wherever it's declared |
| `nixlayer move <package> <category>` | Relocate between categories |
| `nixlayer list [category]` | Show managed packages |
| `nixlayer categories` | List categories and counts |
| `nixlayer where <package>` | Which category owns a package |
| `nixlayer diff` | What would change on the next rebuild |
| `nixlayer rebuild [switch\|boot\|test]` | Validate, then run `nixos-rebuild` |
| `nixlayer doctor` | Health check |

Every mutating command supports `--dry-run`. `add` also supports
`--allow-unfree` (required before an unfree package will be written) and
`--rebuild` (rebuild immediately instead of the default declarative-only
behavior).

## What nixlayer will never do

- Rewrite your whole `configuration.nix`, `hardware-configuration.nix`, or any
  file it doesn't own.
- Silently flip `nixpkgs.config.allowUnfree` for you.
- Guess at ambiguous edits — if it can't confidently and safely make a change,
  it stops and tells you the exact line to add yourself.
- Auto-rebuild without being asked (`nixlayer add` is declarative-only by default).
- Invent nixpkgs package names — every resolved attribute comes from a real
  `nix search`/`nix eval` against nixpkgs, never a hardcoded list.



## Development

```
cargo build
cargo test          
```
> [!Warning]
> use
> ```
> cargo test -- --test-threads=1
> ```
> Since parralel testing is broken right now

Set `NIXLAYER_CONFIG_DIR` to point at any directory to use it instead of
`/etc/nixos` — handy for testing, or for non-standard config layouts.

> [!WARNING]
> This is a passion project of mine, and things are subject to change and break (for sure)
> any feedback is greatly appreciated!
> and also a considerable amount of work is made by ai..
