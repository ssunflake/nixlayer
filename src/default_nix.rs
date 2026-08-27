/// The generated `modules/nixlayer/default.nix`.
///
/// Deliberately self-scanning: it imports every `*.nix` file next to it at
/// eval time via `builtins.readDir`, EXCEPT itself. That means adding a new
/// category (`nixlayer add <pkg> --category <new-name>`) never requires
/// rewriting this file — one less file nixlayer has to edit, one less thing
/// that can go wrong.
///
/// `allow_unfree` optionally adds `nixpkgs.config.allowUnfree = true;` right
/// here. This is the ONE nixpkgs-wide setting nixlayer will toggle on your
/// behalf (via `nixlayer allow-unfree`), and it's done entirely inside this
/// already-owned file — never by touching configuration.nix.
pub fn render(allow_unfree: bool) -> String {
    let unfree_line = if allow_unfree {
        "\n  nixpkgs.config.allowUnfree = true;"
    } else {
        ""
    };
    format!(
        "# This file is managed by nixlayer. Do not hand-edit.\n\
         #\n\
         # It automatically imports every category file next to it (any *.nix file\n\
         # in this directory other than this one). Adding a new category with\n\
         # `nixlayer add <package> --category <name>` does not require touching this file.\n\
         {{ lib, ... }}:\n\
         let\n\
         \x20 dir = ./.;\n\
         \x20 entries = builtins.readDir dir;\n\
         \x20 isCategoryFile = name: type:\n\
         \x20   type == \"regular\" && name != \"default.nix\" && lib.hasSuffix \".nix\" name;\n\
         \x20 categoryFiles = builtins.attrNames (lib.filterAttrs isCategoryFile entries);\n\
         in\n\
         {{\n\
         \x20 imports = map (name: dir + \"/${{name}}\") categoryFiles;{unfree_line}\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_read_dir_and_no_hardcoded_categories() {
        let s = render(false);
        assert!(s.contains("readDir"));
        assert!(!s.contains("app.nix"));
        assert!(!s.contains("gaming.nix"));
    }

    #[test]
    fn omits_allow_unfree_by_default() {
        let s = render(false);
        assert!(!s.contains("allowUnfree"));
    }

    #[test]
    fn includes_allow_unfree_when_enabled() {
        let s = render(true);
        assert!(s.contains("nixpkgs.config.allowUnfree = true;"));
    }
}
