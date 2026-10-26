//! File icons used by the Files panel.

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
