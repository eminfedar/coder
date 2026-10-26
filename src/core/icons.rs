//! File icons used by the Files panel. Well-known languages get their own
//! Nerd Font logo in the language's familiar brand color (Rust orange, C++ blue,
//! JavaScript yellow, ...); everything else a generic codicon by file kind.

use ratatui::style::Color;

const FILE: &str = "\u{ea7b}";
const FILE_CODE: &str = "\u{eae9}";
const MARKDOWN: &str = "\u{eb1d}";
const FILE_MEDIA: &str = "\u{eaea}";
const FILE_PDF: &str = "\u{eaeb}";
const FILE_ARCHIVE: &str = "\u{eaef}";
const FOLDER: &str = "\u{ea83}";
const FOLDER_OPEN: &str = "\u{eaf7}";
const DOCKER: &str = "\u{e7b0}";

/// Returns a one-cell file or folder icon for the non-ASCII explorer view.
pub fn file(name: &str, is_dir: bool, opened: bool) -> &'static str {
    if is_dir {
        return if opened { FOLDER_OPEN } else { FOLDER };
    }
    if let Some((icon, _)) = language(name) {
        return icon;
    }
    match kind(name) {
        FileIconKind::Code => FILE_CODE,
        FileIconKind::Markdown => MARKDOWN,
        FileIconKind::Text => FILE,
        FileIconKind::Media => FILE_MEDIA,
        FileIconKind::Pdf => FILE_PDF,
        FileIconKind::Archive => FILE_ARCHIVE,
        FileIconKind::Docker => DOCKER,
        FileIconKind::Generic => FILE,
    }
}

/// Brand color of a file's language icon; `None` for folders and files without
/// one (drawn in the theme's dim color).
pub fn color(name: &str, is_dir: bool) -> Option<Color> {
    if is_dir {
        return None;
    }
    language(name).map(|(_, (r, g, b))| Color::Rgb(r, g, b))
}

/// `(logo, brand color)` of a well-known language / tool file, by whole name
/// first, then by (case-insensitive) extension.
fn language(name: &str) -> Option<(&'static str, (u8, u8, u8))> {
    let by_name = match name {
        "Dockerfile" | "Containerfile" | ".dockerignore" => Some((DOCKER, (69, 142, 230))),
        "Makefile" | "makefile" | "GNUmakefile" => Some(("\u{e779}", (109, 128, 134))),
        ".gitignore" | ".gitattributes" | ".gitmodules" => Some(("\u{e702}", (241, 76, 40))),
        _ => None,
    };
    if by_name.is_some() {
        return by_name;
    }
    let lower = name.to_ascii_lowercase();
    if matches!(lower.as_str(), "docker-compose.yml" | "docker-compose.yaml") {
        return Some((DOCKER, (69, 142, 230)));
    }
    let ext = lower.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => ("\u{e7a8}", (247, 76, 0)),
        "go" => ("\u{e627}", (0, 173, 216)),
        "c" => ("\u{e61e}", (89, 158, 255)),
        "h" => ("\u{f0fd}", (160, 116, 196)),
        "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" => ("\u{e61d}", (0, 89, 156)),
        "cs" => ("\u{f031b}", (155, 79, 150)),
        "js" | "mjs" | "cjs" => ("\u{e74e}", (241, 224, 90)),
        "ts" | "mts" | "cts" => ("\u{e628}", (49, 120, 198)),
        "jsx" | "tsx" => ("\u{e7ba}", (32, 194, 227)),
        "py" | "pyi" | "pyw" => ("\u{e606}", (55, 118, 171)),
        "java" => ("\u{e738}", (204, 62, 68)),
        "kt" | "kts" => ("\u{e634}", (127, 82, 255)),
        "swift" => ("\u{e755}", (227, 121, 51)),
        "rb" => ("\u{e739}", (204, 52, 45)),
        "php" => ("\u{e73d}", (119, 123, 180)),
        "lua" => ("\u{e620}", (81, 160, 207)),
        "zig" => ("\u{e6a9}", (246, 154, 27)),
        "hs" => ("\u{e61f}", (160, 116, 196)),
        "dart" => ("\u{e798}", (0, 180, 171)),
        "ex" | "exs" => ("\u{e62d}", (160, 116, 196)),
        "scala" => ("\u{e737}", (204, 62, 68)),
        "nix" => ("\u{f313}", (126, 186, 228)),
        "vue" => ("\u{e6a0}", (66, 184, 131)),
        "vim" => ("\u{e62b}", (1, 152, 51)),
        "html" | "htm" => ("\u{e736}", (227, 76, 38)),
        "css" => ("\u{e749}", (66, 165, 245)),
        "scss" | "sass" => ("\u{e603}", (207, 100, 154)),
        "json" | "jsonc" => ("\u{e60b}", (203, 203, 65)),
        "toml" => ("\u{e6b2}", (156, 66, 33)),
        "yaml" | "yml" => ("\u{e6a8}", (203, 23, 30)),
        "sh" | "bash" | "zsh" | "fish" => ("\u{e795}", (137, 224, 81)),
        "sql" => ("\u{e706}", (218, 216, 216)),
        "lock" => ("\u{f023}", (187, 187, 187)),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum FileIconKind {
    Code,
    Markdown,
    Text,
    Media,
    Pdf,
    Archive,
    Docker,
    Generic,
}

fn kind(name: &str) -> FileIconKind {
    let lower = name.to_ascii_lowercase();
    if matches!(name, "Dockerfile" | ".dockerignore")
        || matches!(lower.as_str(), "docker-compose.yml" | "docker-compose.yaml")
    {
        return FileIconKind::Docker;
    }
    match lower.rsplit('.').next().unwrap_or_default() {
        "rs" | "py" | "js" | "jsx" | "ts" | "tsx" | "go" | "java" | "c" | "h" | "cpp" | "hpp"
        | "cs" | "rb" | "php" | "swift" | "kt" | "kts" | "zig" | "lua" | "sh" | "fish" | "vim" => {
            FileIconKind::Code
        }
        "md" | "markdown" => FileIconKind::Markdown,
        "txt" | "rst" | "adoc" | "log" => FileIconKind::Text,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" | "ico" => FileIconKind::Media,
        "pdf" => FileIconKind::Pdf,
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" => FileIconKind::Archive,
        _ => FileIconKind::Generic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_languages_get_their_logo_and_brand_color() {
        assert_eq!(file("main.rs", false, false), "\u{e7a8}");
        assert_eq!(color("main.rs", false), Some(Color::Rgb(247, 76, 0)));
        // Extensions match case-insensitively; whole names win over extensions.
        assert_eq!(file("APP.JS", false, false), "\u{e74e}");
        assert_eq!(file("Dockerfile", false, false), DOCKER);
    }

    #[test]
    fn other_files_keep_the_generic_kind_icons_without_color() {
        assert_eq!(file("README.md", false, false), MARKDOWN);
        assert_eq!(file("notes.xyz", false, false), FILE);
        assert_eq!(color("notes.xyz", false), None);
        assert_eq!(color("src.rs", true), None, "a folder named like a file");
    }
}
