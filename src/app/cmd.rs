//! Side effects (the `Cmd` of the Elm Architecture) and the async executor.

use std::path::PathBuf;

use tokio::sync::mpsc::UnboundedSender;

use crate::app::msg::Msg;
use crate::services;
use crate::services::extensions::{ServerSpec, ToolSpec};
use crate::services::lsp::{LspClientMsg, Token};

/// A side-effect description returned by `update`. The executor runs these on
/// tokio and sends the result back as a `Msg`.
pub enum Cmd {
    ScanDir(PathBuf),
    /// Create a new empty file (`is_dir` = false) or directory, then re-scan its parent.
    CreatePath {
        path: PathBuf,
        is_dir: bool,
    },
    /// Rename a file/directory, then re-scan its parent.
    RenamePath {
        from: PathBuf,
        to: PathBuf,
    },
    /// Delete a file (or a directory and its contents), then re-scan its parent.
    DeletePath(PathBuf),
    ReadFile(PathBuf),
    /// Re-read a file that changed on disk (result -> `Msg::FileReloaded`).
    ReloadFile(PathBuf),
    WriteFile {
        path: PathBuf,
        contents: String,
    },
    /// Load the HEAD content of a file for the change gutter.
    LoadHeadText(PathBuf),
    /// Build the patch of a history commit for its read-only "<hash> diff" tab.
    LoadCommitDiff(String),
    LoadGitStatus,
    GitStage(String),
    GitUnstage(String),
    GitStageAll,
    GitUnstageAll,
    GitRevert(String),
    GitCommit(String),
    /// Soft-reset HEAD~1: undo the last commit, keep its changes staged.
    GitUndoLastCommit,
    GitFetch,
    GitPull,
    GitPush,

    /// Enumerate every workspace file for the quickbar's "search file" list.
    /// Result -> `Msg::FilesListed`.
    ListFiles,
    RunSearch {
        query: String,
        use_regex: bool,
        match_case: bool,
        search_hidden: bool,
    },
    RunReplace {
        query: String,
        replace: String,
        use_regex: bool,
        match_case: bool,
        search_hidden: bool,
    },
    /// Replace only on one result's line (the search panel's "Replace" button).
    RunReplaceLine {
        path: PathBuf,
        line_no: usize,
        query: String,
        replace: String,
        use_regex: bool,
        match_case: bool,
    },
    SpawnPty {
        rows: u16,
        cols: u16,
    },
    /// Start a language server for a language (idempotent per language).
    LspEnsureStarted {
        language: String,
        spec: ServerSpec,
        root: PathBuf,
    },
    /// Send an intent to a running server via its handle's sender.
    LspSend {
        to_server: UnboundedSender<LspClientMsg>,
        msg: LspClientMsg,
    },
    /// Run a standalone formatter (stdin -> stdout) on the buffer text.
    RunFormatterTool {
        path: PathBuf,
        spec: ToolSpec,
        text: String,
        token: Token,
        save_after: bool,
    },
    /// Run a standalone linter (stdin -> stdout) and parse its diagnostics.
    RunLinterTool {
        path: PathBuf,
        spec: ToolSpec,
        text: String,
    },
    SetClipboard(String),
    /// Read the system clipboard off the UI thread (arboard's X11 read can block
    /// waiting on the selection owner) -> `Msg::ClipboardRead`. `fallback` (the
    /// internal clipboard) is used when the system one is empty/unavailable.
    ReadClipboard {
        fallback: String,
    },
    /// Probe whether each named binary is installed on PATH (result ->
    /// `Msg::ToolsChecked`), for the Extensions panel status.
    CheckTools(Vec<String>),
    /// Persist user preferences (theme + settings) to the config file.
    SaveConfig(services::config::Config),
    /// Wake the app after the toast duration so an expired toast is cleared even
    /// without other events arriving.
    ScheduleToastExpiry,
    /// Write a debounced session checkpoint (open tabs, cursor/scroll, unsaved
    /// content) for the current workspace. `seen` is the generation this
    /// instance last observed — see `services::session::save`'s multi-instance
    /// note; result -> `Msg::SessionSaved`.
    SaveSession {
        snapshot: services::session::SessionSnapshot,
        seen: u64,
    },
}

/// Whether a command names an executable that exists: a path with a separator is
/// checked directly, otherwise each `PATH` entry is probed for the file.
fn binary_on_path(command: &str) -> bool {
    if command.contains('/') {
        return std::path::Path::new(command).is_file();
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(command).is_file())
}

/// A pending file write for the single writer task.
type WriteJob = (PathBuf, String, UnboundedSender<Msg>);

/// Queues a file write. Every save goes through one long-lived writer task, so
/// two quick saves of the same file land on disk in the order they were issued
/// (independent `tokio::spawn`s could finish in either order, leaving the older
/// text on disk). Replies with `Msg::FileSaved` carrying the written text.
fn queue_write(path: PathBuf, contents: String, tx: UnboundedSender<Msg>) {
    static WRITER: std::sync::OnceLock<UnboundedSender<WriteJob>> = std::sync::OnceLock::new();
    let writer = WRITER.get_or_init(|| {
        let (jobs_tx, mut jobs) = tokio::sync::mpsc::unbounded_channel::<WriteJob>();
        tokio::spawn(async move {
            while let Some((path, contents, tx)) = jobs.recv().await {
                match services::fs::write_file(&path, &contents).await {
                    Ok(()) => {
                        let _ = tx.send(Msg::FileSaved { path, contents });
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("could not save: {e}")));
                    }
                }
            }
        });
        jobs_tx
    });
    let _ = writer.send((path, contents, tx));
}

/// Longest a formatter/linter may run before it is killed.
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Runs a stdin->stdout tool: feeds `input` on stdin, returns its output.
///
/// stdin is written and stdout/stderr drained on their own threads, so a tool
/// that streams output while still reading can never fill a pipe and deadlock
/// against us; a hung tool is killed after `TOOL_TIMEOUT`.
fn run_tool(spec: &ToolSpec, input: &str) -> std::io::Result<std::process::Output> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let mut child = Command::new(&spec.command)
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let writer = child.stdin.take().map(|mut stdin| {
        let input = input.as_bytes().to_vec();
        // stdin drops at the end of the thread, closing the pipe so the tool sees EOF.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        })
    });
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut out);
            }
            out
        })
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let deadline = std::time::Instant::now() + TOOL_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{} timed out", spec.command),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    Ok(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Parses common `path:line:col: message` linter output into `(line0, col0, msg)`.
fn parse_linter_output(text: &str) -> Vec<(usize, usize, String)> {
    // Matches the first `line:col` pair on a line, with an optional message tail.
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(\d+):(\d+):?\s*(.*)").unwrap());
    let re = &*RE;
    text.lines()
        .filter_map(|line| {
            let caps = re.captures(line)?;
            let ln: usize = caps.get(1)?.as_str().parse().ok()?;
            let col: usize = caps.get(2)?.as_str().parse().ok()?;
            let msg = caps
                .get(3)
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string();
            // Linter positions are 1-based; store 0-based.
            Some((ln.saturating_sub(1), col.saturating_sub(1), msg))
        })
        .collect()
}

/// The first non-empty line of a message, for the one-line status bar.
fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Re-scans the directory holding `path` so the file tree picks up a create,
/// rename or delete.
fn rescan_parent(path: &std::path::Path, tx: &UnboundedSender<Msg>) {
    let Some(dir) = path.parent().map(PathBuf::from) else {
        return;
    };
    match services::fs::scan_dir(&dir) {
        Ok(entries) => {
            let _ = tx.send(Msg::DirScanned { path: dir, entries });
        }
        Err(e) => {
            let _ = tx.send(Msg::Error(format!("could not scan directory: {e}")));
        }
    }
}

/// Loads the git status and sends `Msg::GitStatusLoaded`.
///
/// Loads are coalesced: while one runs, further requests only flag a rerun, so
/// a burst of saves / disk events costs at most one extra `git status`. Since
/// one thread at a time loads and sends, results also arrive in order — an
/// older status can never overwrite a newer one.
fn send_git_status(root: &std::path::Path, tx: &UnboundedSender<Msg>) {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static RERUN: AtomicBool = AtomicBool::new(false);
    // The newest request's root and channel (the workspace root can change).
    static LATEST: Mutex<Option<(PathBuf, UnboundedSender<Msg>)>> = Mutex::new(None);

    if let Ok(mut latest) = LATEST.lock() {
        *latest = Some((root.to_path_buf(), tx.clone()));
    }
    RERUN.store(true, SeqCst);
    if RUNNING.swap(true, SeqCst) {
        return; // the running loader will see RERUN and go again
    }
    loop {
        RERUN.store(false, SeqCst);
        let job = LATEST.lock().ok().and_then(|l| l.clone());
        if let Some((root, tx)) = job {
            load_and_send_git_status(&root, &tx);
        }
        RUNNING.store(false, SeqCst);
        // A request that raced the store above either restarted us here, or
        // won `RUNNING` itself and runs its own loop.
        if !RERUN.load(SeqCst) || RUNNING.swap(true, SeqCst) {
            break;
        }
    }
}

fn load_and_send_git_status(root: &std::path::Path, tx: &UnboundedSender<Msg>) {
    let status = services::git::load_status(root);
    let _ = tx.send(Msg::GitStatusLoaded(status));
}

/// Runs a Cmd; results come back as Msg over `tx`.
pub fn execute(cmd: Cmd, root: PathBuf, tx: UnboundedSender<Msg>) {
    match cmd {
        Cmd::ScanDir(path) => {
            tokio::task::spawn_blocking(move || match services::fs::scan_dir(&path) {
                Ok(entries) => {
                    let _ = tx.send(Msg::DirScanned { path, entries });
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("could not scan directory: {e}")));
                }
            });
        }
        Cmd::CreatePath { path, is_dir } => {
            tokio::task::spawn_blocking(move || {
                let result = if is_dir {
                    services::fs::create_dir(&path)
                } else {
                    services::fs::create_file(&path)
                };
                match result {
                    Ok(()) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let _ = tx.send(Msg::Toast(format!("Created File '{name}'")));
                        rescan_parent(&path, &tx);
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("could not create: {e}")));
                    }
                }
            });
        }
        Cmd::RenamePath { from, to } => {
            tokio::task::spawn_blocking(move || {
                match services::fs::rename_path(&from, &to) {
                    Ok(()) => {
                        rescan_parent(&to, &tx);
                        // Open tabs under the old path follow it; update does the rest.
                        let _ = tx.send(Msg::PathRenamed { from, to });
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("could not rename: {e}")));
                    }
                }
            });
        }
        Cmd::DeletePath(path) => {
            tokio::task::spawn_blocking(move || match services::fs::delete_path(&path) {
                Ok(()) => {
                    rescan_parent(&path, &tx);
                    let _ = tx.send(Msg::PathDeleted(path));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("could not delete: {e}")));
                }
            });
        }
        Cmd::ReadFile(path) => {
            tokio::spawn(async move {
                match tokio::fs::read(&path).await {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => {
                            let _ = tx.send(Msg::FileLoaded { path, text });
                        }
                        // Invalid UTF-8 -> a binary file we cannot display as text.
                        Err(_) => {
                            let _ = tx.send(Msg::FileLoadFailed {
                                path,
                                error: "This file cannot be displayed because it is a binary file or uses an unsupported encoding.".to_string(),
                            });
                        }
                    },
                    Err(e) => {
                        let _ = tx.send(Msg::FileLoadFailed {
                            path,
                            error: format!("Could not open file: {e}"),
                        });
                    }
                }
            });
        }
        Cmd::ReloadFile(path) => {
            tokio::spawn(async move {
                if let Ok(text) = services::fs::read_file(&path).await {
                    let _ = tx.send(Msg::FileReloaded { path, text });
                }
            });
        }
        Cmd::WriteFile { path, contents } => {
            queue_write(path, contents, tx);
        }
        Cmd::LoadHeadText(path) => {
            tokio::task::spawn_blocking(move || {
                let text = services::git::head_file(&path);
                let _ = tx.send(Msg::HeadTextLoaded { path, text });
            });
        }
        Cmd::LoadCommitDiff(hash) => {
            tokio::task::spawn_blocking(move || match services::git::commit_diff(&root, &hash) {
                Ok(diff) => {
                    let _ = tx.send(Msg::CommitDiffLoaded { hash, diff });
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("could not load commit diff: {e}")));
                }
            });
        }
        Cmd::LoadGitStatus => {
            tokio::task::spawn_blocking(move || {
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitStage(rel) => {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = services::git::stage(&root, &rel) {
                    let _ = tx.send(Msg::Error(format!("stage failed: {e}")));
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitUnstage(rel) => {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = services::git::unstage(&root, &rel) {
                    let _ = tx.send(Msg::Error(format!("unstage failed: {e}")));
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitStageAll => {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = services::git::stage_all(&root) {
                    let _ = tx.send(Msg::Error(format!("stage all failed: {e}")));
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitUnstageAll => {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = services::git::unstage_all(&root) {
                    let _ = tx.send(Msg::Error(format!("unstage all failed: {e}")));
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitRevert(rel) => {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = services::git::revert(&root, &rel) {
                    let _ = tx.send(Msg::Error(format!("revert failed: {e}")));
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitCommit(message) => {
            tokio::task::spawn_blocking(move || {
                match services::git::commit(&root, &message) {
                    Ok(()) => {
                        let _ = tx.send(Msg::GitCommitted);
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("commit failed: {e}")));
                    }
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitUndoLastCommit => {
            tokio::task::spawn_blocking(move || {
                match services::git::undo_last_commit(&root) {
                    Ok(message) => {
                        let _ = tx.send(Msg::GitCommitUndone { message });
                        let _ = tx.send(Msg::Toast("Undid last commit".to_string()));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("undo commit failed: {e}")));
                    }
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitFetch => {
            tokio::task::spawn_blocking(move || {
                match services::git::fetch(&root) {
                    Ok(_) => {
                        let _ = tx.send(Msg::Toast("Fetched".to_string()));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Toast(format!("fetch failed: {}", first_line(&e))));
                    }
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitPull => {
            tokio::task::spawn_blocking(move || {
                match services::git::pull(&root) {
                    Ok(m) => {
                        let _ = tx.send(Msg::Toast(format!("Pulled: {}", first_line(&m))));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Toast(format!("pull failed: {}", first_line(&e))));
                    }
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::GitPush => {
            tokio::task::spawn_blocking(move || {
                match services::git::push(&root) {
                    Ok(_) => {
                        let _ = tx.send(Msg::Toast("Pushed".to_string()));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Toast(format!("push failed: {}", first_line(&e))));
                    }
                }
                send_git_status(&root, &tx);
            });
        }
        Cmd::ListFiles => {
            let root = root.clone();
            tokio::task::spawn_blocking(move || {
                let paths = services::search::list_files(&root);
                let _ = tx.send(Msg::FilesListed { paths });
            });
        }
        Cmd::RunSearch {
            query,
            use_regex,
            match_case,
            search_hidden,
        } => {
            tokio::task::spawn_blocking(move || {
                let matches = services::search::search(
                    &root,
                    &query,
                    use_regex,
                    match_case,
                    search_hidden,
                    500,
                );
                let _ = tx.send(Msg::SearchResults { query, matches });
            });
        }
        Cmd::RunReplace {
            query,
            replace,
            use_regex,
            match_case,
            search_hidden,
        } => {
            tokio::task::spawn_blocking(move || {
                let (changed, count) = services::search::replace_all(
                    &root,
                    &query,
                    &replace,
                    use_regex,
                    match_case,
                    search_hidden,
                );
                let _ = tx.send(Msg::ReplaceDone { changed, count });
            });
        }
        Cmd::RunReplaceLine {
            path,
            line_no,
            query,
            replace,
            use_regex,
            match_case,
        } => {
            tokio::task::spawn_blocking(move || {
                let count = services::search::replace_in_line(
                    &path, line_no, &query, &replace, use_regex, match_case,
                );
                let changed = if count > 0 { vec![path] } else { Vec::new() };
                let _ = tx.send(Msg::ReplaceDone { changed, count });
            });
        }
        Cmd::SpawnPty { rows, cols } => {
            tokio::task::spawn_blocking(move || {
                match services::pty::spawn(root, rows, cols, tx.clone()) {
                    Ok(session) => {
                        let _ = tx.send(Msg::PtyReady(session));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("could not start terminal: {e}")));
                    }
                }
            });
        }
        Cmd::LspEnsureStarted {
            language,
            spec,
            root: server_root,
        } => {
            let handle = services::lsp::start(language.clone(), spec, server_root, tx.clone());
            let _ = tx.send(Msg::LspSessionReady { language, handle });
        }
        Cmd::LspSend { to_server, msg } => {
            // Sending on the unbounded intent channel is non-blocking.
            let _ = to_server.send(msg);
        }
        Cmd::RunFormatterTool {
            path,
            spec,
            text,
            token,
            save_after,
        } => {
            tokio::task::spawn_blocking(move || match run_tool(&spec, &text) {
                Ok(out) if out.status.success() => {
                    let formatted = String::from_utf8_lossy(&out.stdout).to_string();
                    let _ = tx.send(Msg::FormatterOutput {
                        path,
                        text: formatted,
                        token,
                        save_after,
                    });
                }
                result => {
                    let detail = match &result {
                        Ok(out) => first_line(&String::from_utf8_lossy(&out.stderr)),
                        Err(e) => e.to_string(),
                    };
                    let _ = tx.send(Msg::Toast(format!("formatter failed: {detail}")));
                    // Don't lose the user's save: write the original text.
                    if save_after {
                        queue_write(path, text, tx);
                    }
                }
            });
        }
        Cmd::RunLinterTool { path, spec, text } => {
            tokio::task::spawn_blocking(move || {
                if let Ok(out) = run_tool(&spec, &text) {
                    let combined = format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                    let items = parse_linter_output(&combined);
                    let _ = tx.send(Msg::LinterDiagnostics { path, items });
                }
            });
        }
        Cmd::ReadClipboard { fallback } => {
            tokio::task::spawn_blocking(move || {
                let text = services::clipboard::get_text();
                let text = if text.is_empty() { fallback } else { text };
                let _ = tx.send(Msg::ClipboardRead(text));
            });
        }
        Cmd::SetClipboard(text) => {
            // OSC-52 must be written on this (main) thread: stdout is owned by the
            // crossterm TUI, and writing it from a background task could interleave
            // bytes with a concurrent render. It asks the *local* terminal (through
            // SSH / tmux / a remote that has no display) to copy into its own
            // clipboard, which is what makes copy reach the user's Windows host.
            services::clipboard::emit_osc52(&text);
            // The native system clipboard (X11/Wayland) is a separate channel that
            // only works on a local desktop; it may be slow (a wayland round-trip)
            // so keep it off the render thread.
            let text2 = text;
            tokio::task::spawn_blocking(move || {
                services::clipboard::set_system(&text2);
            });
        }
        Cmd::CheckTools(commands) => {
            tokio::task::spawn_blocking(move || {
                let statuses = commands
                    .into_iter()
                    .map(|c| {
                        let installed = binary_on_path(&c);
                        (c, installed)
                    })
                    .collect();
                let _ = tx.send(Msg::ToolsChecked(statuses));
            });
        }
        Cmd::SaveConfig(config) => {
            // Arrowing through the Themes panel saves on every step. Writers are
            // serialized and each writes the *newest* pending config, so a
            // burst collapses to a write or two and the last change always
            // lands last (independent tasks could finish in any order).
            use std::sync::Mutex;
            static PENDING: Mutex<Option<services::config::Config>> = Mutex::new(None);
            static WRITE: Mutex<()> = Mutex::new(());
            if let Ok(mut p) = PENDING.lock() {
                *p = Some(config);
            }
            let tx = tx.clone();
            tokio::task::spawn_blocking(move || {
                let _serial = WRITE.lock();
                let latest = PENDING.lock().ok().and_then(|mut p| p.take());
                if let Some(config) = latest {
                    if let Err(e) = services::config::save(&config) {
                        let _ = tx.send(Msg::Toast(format!("Could not save config.toml: {e}")));
                    }
                }
            });
        }
        Cmd::ScheduleToastExpiry => {
            tokio::spawn(async move {
                tokio::time::sleep(crate::app::model::TOAST_DURATION).await;
                let _ = tx.send(Msg::ToastExpired);
            });
        }
        Cmd::SaveSession { mut snapshot, seen } => {
            tokio::task::spawn_blocking(move || {
                let outcome = services::session::save(&root, &mut snapshot, seen);
                let _ = tx.send(Msg::SessionSaved(outcome));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_linter_output;

    #[test]
    fn run_tool_pipes_stdin_to_stdout() {
        use crate::services::extensions::ToolSpec;
        // `tr a-z A-Z` uppercases stdin — a deterministic stand-in for a formatter.
        if std::process::Command::new("tr")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let spec = ToolSpec {
            command: "tr".to_string(),
            args: vec!["a-z".to_string(), "A-Z".to_string()],
        };
        let out = super::run_tool(&spec, "hello").unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "HELLO");
    }

    #[test]
    fn parses_ruff_style_output() {
        let out =
            "app.py:3:5: F401 unused import\napp.py:10:1: E302 expected 2 blank lines\nnoise line";
        let items = parse_linter_output(out);
        assert_eq!(items.len(), 2);
        // 1-based input -> 0-based storage.
        assert_eq!(items[0], (2, 4, "F401 unused import".to_string()));
        assert_eq!(items[1].0, 9);
    }
}
