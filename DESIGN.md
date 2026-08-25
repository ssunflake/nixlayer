# nixlayer — design notes

## 1. Architecture

```
/etc/nixos/
├── flake.nix                    <- untouched
├── configuration.nix            <- one import line added, once, at init
└── modules/
    └── nixlayer/                  <- nixlayer's entire footprint
        ├── default.nix            <- self-scanning importer, generated once
        ├── .nixlayer-state.json   <- last-successful-rebuild snapshot (for `diff`)
        ├── app.nix                <- default category
        ├── gaming.nix
        └── development.nix
```

**Ownership boundary**: nixlayer only ever reads/writes inside `modules/nixlayer/`,
plus one narrowly-scoped edit to `configuration.nix`'s `imports = [ ... ];`
list at `init` time. Every other file on the system is out of bounds,
enforced structurally (there is no code path in nixlayer that opens an arbitrary
path the user gives it for *writing* — `paths.rs` is the only place file
paths are constructed, and it only ever builds paths under `nixlayer_dir`).

**Why one file per category, not one file per package**: a `default.nix` that
did `imports = [ ./firefox.nix ./vlc.nix ./steam.nix ... ]` would technically
work, but it turns a 40-package system into 40 tiny files and pushes
"grouping" into naming conventions. A category file is a normal, small,
readable Nix module — exactly what you'd write by hand, just kept in sync by
a tool.

**Why `default.nix` self-scans (`builtins.readDir`) instead of listing
category files explicitly**: it means adding a *new* category never requires
nixlayer to modify `default.nix` — one less owned file that changes, one less
place a concurrent edit or crash could corrupt. The generated file is:

```nix
{ lib, ... }:
let
  dir = ./.;
  entries = builtins.readDir dir;
  isCategoryFile = name: type:
    type == "regular" && name != "default.nix" && lib.hasSuffix ".nix" name;
  categoryFiles = builtins.attrNames (lib.filterAttrs isCategoryFile entries);
in
{ imports = map (name: dir + "/${name}") categoryFiles; }
```

**Why nixlayer doesn't touch flake inputs/outputs**: ordinary nixpkgs packages
are already reachable through the `pkgs` argument every NixOS module
receives. There is no reason for a package-list manager to negotiate flake
inputs for `pkgs.firefox`. This is reserved for genuinely flake-shaped future
features (arbitrary flake packages, NUR) — explicitly out of scope for v0.1.

## 2. Category file format

Fixed, generated template; the *only* thing nixlayer parses/rewrites inside it
is the `environment.systemPackages = with pkgs; [ ... ];` list:

```nix
# This file is managed by nixlayer.
# Category: gaming
# Do not hand-edit the package list unless you know what you're doing —
# nixlayer may reorder or rewrite it when you run `nixlayer add/remove/move`.
# Anything other than the environment.systemPackages list below is left alone.

{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    mangohud
    prismlauncher
    steam
  ];
}
```

Parsing strategy (`src/nixfile.rs`): nixlayer does **not** contain a Nix
parser. It does a bracket-depth scan to find the `[ ... ]` following
`environment.systemPackages`, then tokenizes the inside by stripping `#`
comments and splitting on whitespace. Every token must match a plain
attribute-path grammar (`firefox`, `nodePackages.typescript`). If a category
file contains anything else — e.g. someone hand-added
`(python3.withPackages (p: [ p.numpy ]))` — nixlayer refuses to auto-manage
that file (`UnparseableCategoryFile`) rather than risk mangling it. This is
the load-bearing safety property of the whole design: *nixlayer only edits what
it fully understands, and stops otherwise.*

Every generated/edited file is also run through `nix-instantiate --parse`
(falling back to `nix eval --file ... --apply 'x: null'`) when available, as
a real-Nix-implementation syntax check — not a homemade one. If that's not
installed, the check is skipped and `doctor` says so explicitly rather than
silently pretending everything's fine.

## 3. `configuration.nix` editing

The only edit nixlayer ever makes outside its own directory: inserting
`./modules/nixlayer/default.nix` into the first `imports = [ ... ];` block it
can find with high confidence. Safety rules:

- If the import line is already present anywhere in the file: no-op (idempotent).
- If a *second* top-level `imports = [` is found: refuse and print the exact
  line to add by hand. Ambiguity is a stop condition, not a "pick the first
  one" condition.
- A timestamped backup (`configuration.nix.bak-<epoch>`) is written before
  any edit.
- The result is syntax-checked; on failure, the original file is restored
  immediately and the command reports failure — an edit is never left
  half-applied.
- If there's no `configuration.nix` at all (e.g. flake-only layouts that
  structure things differently), nixlayer does not guess where else to plug in
  — it prints the one line the user needs to add themselves.

## 4. Package resolution — the part that actually matters

nixlayer never invents nixpkgs attribute names. It shells out to the real Nix
toolchain and reads back real data. Two backends, chosen automatically:

### Flakes backend (preferred)

- `nix eval --extra-experimental-features 'nix-command flakes' --json nixpkgs#<attr> --apply <expr>`
  reduces a resolved derivation to a small JSON record (pname, version,
  description, homepage, license list, `free`, `broken`), using `or null` /
  `or true` everywhere so it degrades gracefully instead of throwing across
  nixpkgs versions.
- `nix search nixpkgs <query> --json` for free-text search, used when the
  query isn't already a valid top-level attribute.
- The `--extra-experimental-features` flag is passed on every invocation so
  this works even on systems where flakes aren't enabled in `nix.conf` — no
  change to the user's Nix configuration is required.

### Legacy backend (fallback)

If `nix` isn't on `PATH` at all (older Nix, no unified CLI) but `nix-env` is,
nixlayer falls back to `nix-env -qaP --json` / `nix-env -qa --json -A
nixpkgs.<attr>` against the active channel. This still resolves real
attribute names, but with **reduced metadata** — no license or homepage data
is available this way, and `doctor`/`info` say so explicitly rather than
fabricating it.

### Known limitation (stated, not hidden)

Search/eval resolve against whatever the `nixpkgs` flake registry entry (or
active channel, in legacy mode) points to on the machine running nixlayer —
which is not guaranteed to be byte-identical to whatever nixpkgs your own
`flake.nix` pins. In practice this essentially never matters for *attribute
names* (they're extremely stable across nixpkgs revisions), but it means the
exact version/description shown by `search`/`info` can drift slightly from
what actually gets built. This is fine because **nixlayer never writes a
resolved value into your config — only the attribute name** (e.g. `steam`).
The actual build always goes through your own `pkgs` at `nixos-rebuild` time,
via the normal module system, using whatever nixpkgs you've pinned. nixlayer
never bypasses that.

A future version could evaluate against the user's own flake
(`nix eval .#nixosConfigurations.<host>.pkgs.<attr>`) for perfect fidelity,
but that requires knowing the hostname/flake output name and is much slower
(it forces evaluating the whole system closure just to look up one package)
— deliberately deferred.

## 5. Unfree packages

`meta.license.free` (or, for a list of licenses, "all of them free") from the
eval above determines `PackageInfo::free`. If a package is unfree:

- `nixlayer add` refuses by default and prints the exact
  `nixpkgs.config.allowUnfree = true;` line the user would need to add to
  *their own* configuration — nixlayer never edits that setting itself.
- `--allow-unfree` overrides the refusal for that one `add`, but still only
  writes the package attribute — never touches `allowUnfree`.
- `nixlayer info` always shows license + free/unfree status up front.

## 6. Duplicates

`find_duplicates` loads every category file and counts which categories each
resolved package attribute appears in. More than one is a conflict. This
check runs in `doctor` and is a hard gate in `rebuild` (`rebuild::validate`
refuses before ever invoking `nixos-rebuild`). `where <pkg>` and `move <pkg>
<category>` are the suggested fixes, and `move` on a package that's a
duplicate across N categories removes it from all N and adds it once to the
target — self-healing the exact case that caused the conflict.

## 7. Diff / dry-run

Two distinct mechanisms:

- **`--dry-run`** on `add`/`remove`/`move`/`init`: computes and prints the
  change in memory, never touches disk. Implemented by doing the real
  in-memory mutation and rendering it, just skipping the final `fs::write`.
- **`nixlayer diff`**: compares the *current* declared state (freshly read from
  every category file) against `.nixlayer-state.json`, a snapshot written only
  after a successful `nixlayer rebuild`. This answers "what would change if I
  rebuilt right now" without needing to interrogate `/run/current-system` or
  any other NixOS-version-specific internal — nixlayer owns the file it diffs
  against, same as everything else it manages.

## 8. Rebuild

`nixlayer rebuild [switch|boot|test] [--dry-run]`:

1. `require_initialized`
2. Reject on any duplicate declarations (see above).
3. Every category file must parse (no `UnparseableCategoryFile`).
4. Every managed file is syntax-checked via `nix-instantiate --parse` (or the
   `nix eval --apply` fallback) when available.
5. Only then: shell out to the system's own `nixos-rebuild <mode>` —
   nixlayer has zero reimplementation of what that does. On failure, the
   previous generation is untouched (that's `nixos-rebuild`'s own guarantee,
   not something nixlayer adds).
6. Only on a genuinely successful rebuild: snapshot current state to
   `.nixlayer-state.json` for future `diff`s.

`add --rebuild` is supported as a convenience but the default for every
mutating command remains declarative-only, per the spec.

## 9. Explicitly out of scope for v0.1

GUI/TUI, Home Manager, NUR, arbitrary GitHub/flake package installs,
multi-machine sync, profiles, package recommendations, automatic git commits,
general-purpose Nix expression editing. The module boundaries above (one
resolver module, one category-file module, no flake-input handling anywhere)
are meant to make these additive rather than requiring a rewrite:

- **NUR / flake packages**: would add a second `Backend` variant and a
  `source:` annotation stored per package (category files would need a small,
  explicit extension — e.g. a second list for non-nixpkgs sources — rather
  than silently mixing attribute namespaces).
- **Home Manager**: a parallel `modules/nixlayer/home/` tree with its own
  category files and its own self-scanning `default.nix`, reusing all of
  `category.rs` and `nixfile.rs` unchanged.
- **Profiles**: an extra path segment (`modules/nixlayer/<profile>/...`),
  changing `Paths` construction only.
- **TUI**: a new frontend over the existing `commands.rs` functions, which
  already return structured data before any printing happens.

## 10. What was verified vs. what couldn't be, in this environment

This was built and tested in a plain Ubuntu container with no `nix` installed
(network policy doesn't allow fetching the Nix installer here). What *was*
verified directly:

- `cargo test`: 25 unit tests covering category parsing/rendering/round-trips,
  bracket-scanning edge cases (nested brackets, complex/unparseable entries),
  `configuration.nix` patching (simple case, idempotency, ambiguous-imports
  refusal, dry-run, backup creation), duplicate detection, and diff/state
  computation.
- A full manual walkthrough of the compiled binary against a fake
  `/etc/nixos`-shaped directory (via `NIXLAYER_CONFIG_DIR`): `init` (including
  the generated `configuration.nix` diff), `list`, `categories`, `where`,
  `move` (including de-duplicating a package found in two categories),
  `remove`, `doctor` (both clean and with injected problems), and `rebuild
  --dry-run`'s validation gate (correctly refuses on duplicates, correctly
  passes once resolved, correct process exit codes throughout).

What could **not** be verified here, for lack of a real Nix installation:
an actual `nix search`/`nix eval` round-trip against real nixpkgs (`search
firefox`, `add steam`, `info vscode`, a genuinely nonexistent package), and
an actual `nixos-rebuild` invocation. The resolver code path is written
defensively (backend auto-detection, graceful `or null`/`or true` eval
expression, explicit error messages distinguishing "nix not found" / "attribute
not found" / "eval failed") and unit-tested where it doesn't require a live
Nix, but it should be smoke-tested against a real NixOS machine before being
trusted as-is:

```
nixlayer search firefox
nixlayer add firefox
nixlayer add steam --category gaming     # should report unfree, refuse, then succeed with --allow-unfree
nixlayer add vscode
nixlayer add this-package-does-not-exist # should fail with a clear message
nixlayer doctor
nixlayer rebuild --dry-run
```
