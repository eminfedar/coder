//! Repo status and stage/unstage/revert/commit operations via git2.

use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{Repository, Status, StatusOptions};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitState {
    Added,
    Modified,
    Deleted,
    Untracked,
    Renamed,
    Conflicted,
}

impl GitState {
    pub fn short(&self) -> char {
        match self {
            GitState::Added => 'A',
            GitState::Modified => 'M',
            GitState::Deleted => 'D',
            GitState::Untracked => 'U',
            GitState::Renamed => 'R',
            GitState::Conflicted => 'C',
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitEntry {
    pub path: PathBuf,
    pub rel: String,
    pub state: GitState,
}

/// A previous commit shown in the HISTORY section of the Git panel.
#[derive(Clone, Debug)]
pub struct GitCommit {
    /// Abbreviated commit hash: the shortest unique prefix (7+ chars).
    pub hash: String,
    /// First line of the commit message.
    pub summary: String,
}

#[derive(Default, Clone)]
pub struct GitStatus {
    pub branch: Option<String>,
    /// Changes added to the index (staged).
    pub staged: Vec<GitEntry>,
    /// Changes in the working tree (unstaged).
    pub unstaged: Vec<GitEntry>,
    pub is_repo: bool,
    /// Commits the local branch is ahead of its upstream.
    pub ahead: usize,
    /// Commits the local branch is behind its upstream.
    pub behind: usize,
    /// Whether the current branch has a configured upstream.
    pub has_upstream: bool,
    /// Whether the repository has at least one remote configured.
    pub has_remote: bool,
    /// Recent commits (newest first) shown under the HISTORY heading.
    pub history: Vec<GitCommit>,
}

/// Number of recent commits loaded for the HISTORY section.
const HISTORY_LIMIT: usize = 30;

/// Collects the status of the git repository under `root` (blocking; call inside spawn_blocking).
pub fn load_status(root: &Path) -> GitStatus {
    let repo = match Repository::discover(root) {
        Ok(r) => r,
        Err(_) => return GitStatus::default(),
    };

    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()));

    let workdir = repo
        .workdir()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root.to_path_buf());

    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
        for entry in statuses.iter() {
            let Some(rel) = entry.path() else { continue };
            let s = entry.status();
            let path = workdir.join(rel);
            // The same file can be both staged and unstaged (partial stage).
            if let Some(state) = classify_index(s) {
                staged.push(GitEntry {
                    path: path.clone(),
                    rel: rel.to_string(),
                    state,
                });
            }
            if let Some(state) = classify_worktree(s) {
                unstaged.push(GitEntry {
                    path,
                    rel: rel.to_string(),
                    state,
                });
            }
        }
    }

    let (ahead, behind, has_upstream) = ahead_behind(&repo);
    let has_remote = repo.remotes().map(|r| !r.is_empty()).unwrap_or(false);
    let history = load_history(&repo);

    GitStatus {
        branch,
        staged,
        unstaged,
        is_repo: true,
        ahead,
        behind,
        has_upstream,
        has_remote,
        history,
    }
}

/// Walks back from HEAD collecting up to `HISTORY_LIMIT` recent commits (newest first).
fn load_history(repo: &Repository) -> Vec<GitCommit> {
    let mut walk = match repo.revwalk() {
        Ok(w) => w,
        Err(_) => return Vec::new(),
    };
    if walk.push_head().is_err() {
        return Vec::new(); // no HEAD yet (empty repo)
    }
    let mut out = Vec::new();
    for oid in walk.flatten().take(HISTORY_LIMIT) {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        // The shortest *unique* abbreviation (at least `core.abbrev`, 7 by
        // default): the hash is later resolved with `revparse_single`, where a
        // plain 7-char prefix could be ambiguous in a large repository.
        let hash = commit
            .as_object()
            .short_id()
            .ok()
            .and_then(|b| b.as_str().map(str::to_string))
            .unwrap_or_else(|| oid.to_string());
        let summary = commit.summary().unwrap_or("").to_string();
        out.push(GitCommit { hash, summary });
    }
    out
}

/// Ahead/behind commit counts vs the upstream, and whether an upstream is set.
/// Local only (no network); the counts reflect the last fetch.
fn ahead_behind(repo: &Repository) -> (usize, usize, bool) {
    let head = match repo.head() {
        Ok(h) if h.is_branch() => h,
        _ => return (0, 0, false),
    };
    let Some(local_oid) = head.target() else {
        return (0, 0, false);
    };
    let branch = git2::Branch::wrap(head);
    let Ok(upstream) = branch.upstream() else {
        return (0, 0, false);
    };
    let Some(up_oid) = upstream.get().target() else {
        return (0, 0, true);
    };
    match repo.graph_ahead_behind(local_oid, up_oid) {
        Ok((a, b)) => (a, b, true),
        Err(_) => (0, 0, true),
    }
}

/// Status of the index (staged) side.
fn classify_index(s: Status) -> Option<GitState> {
    if s.is_index_new() {
        Some(GitState::Added)
    } else if s.is_index_deleted() {
        Some(GitState::Deleted)
    } else if s.is_index_renamed() {
        Some(GitState::Renamed)
    } else if s.is_index_modified() || s.is_index_typechange() {
        Some(GitState::Modified)
    } else {
        None
    }
}

/// Status of the working tree (unstaged) side.
fn classify_worktree(s: Status) -> Option<GitState> {
    if s.is_conflicted() {
        Some(GitState::Conflicted)
    } else if s.is_wt_new() {
        Some(GitState::Untracked)
    } else if s.is_wt_deleted() {
        Some(GitState::Deleted)
    } else if s.is_wt_renamed() {
        Some(GitState::Renamed)
    } else if s.is_wt_modified() || s.is_wt_typechange() {
        Some(GitState::Modified)
    } else {
        None
    }
}

/// Adds the file to the index (git add). If deleted, removes it from the index.
pub fn stage(root: &Path, rel: &str) -> Result<(), git2::Error> {
    let repo = Repository::discover(root)?;
    let workdir = repo.workdir().map(|p| p.to_path_buf());
    let mut index = repo.index()?;
    let rel_path = Path::new(rel);
    let exists = workdir.map(|w| w.join(rel).exists()).unwrap_or(false);
    if exists {
        index.add_path(rel_path)?;
    } else {
        index.remove_path(rel_path)?;
    }
    index.write()
}

/// Adds all changes to the index (git add -A).
pub fn stage_all(root: &Path) -> Result<(), git2::Error> {
    let repo = Repository::discover(root)?;
    let mut index = repo.index()?;
    // With the "*" pathspec, adds new/changed files and removes deleted ones from the index.
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None)?;
    index.write()
}

/// Removes all staged changes from the index (git reset).
pub fn unstage_all(root: &Path) -> Result<(), git2::Error> {
    let repo = Repository::discover(root)?;
    match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
        Some(commit) => {
            // Collect the staged paths and reset them to HEAD.
            let mut opts = StatusOptions::new();
            opts.include_untracked(false);
            let paths: Vec<String> = repo
                .statuses(Some(&mut opts))?
                .iter()
                .filter(|e| classify_index(e.status()).is_some())
                .filter_map(|e| e.path().map(|p| p.to_string()))
                .collect();
            if !paths.is_empty() {
                repo.reset_default(Some(commit.as_object()), paths.iter())?;
            }
        }
        None => {
            // No HEAD: clear the index.
            let mut index = repo.index()?;
            index.remove_all(["*"], None)?;
            index.write()?;
        }
    }
    Ok(())
}

/// Removes the file from the index (git reset -- <file>).
pub fn unstage(root: &Path, rel: &str) -> Result<(), git2::Error> {
    let repo = Repository::discover(root)?;
    let rel_path = Path::new(rel);
    match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
        Some(commit) => {
            repo.reset_default(Some(commit.as_object()), [rel_path])?;
        }
        None => {
            // No HEAD (no first commit yet): remove from the index.
            let mut index = repo.index()?;
            index.remove_path(rel_path)?;
            index.write()?;
        }
    }
    Ok(())
}

/// Reverts the change in the working tree (git checkout -- <file> / delete untracked).
pub fn revert(root: &Path, rel: &str) -> Result<(), git2::Error> {
    let repo = Repository::discover(root)?;
    let rel_path = Path::new(rel);
    let status = repo.status_file(rel_path)?;
    if status.is_wt_new() {
        // Untracked new file: delete it from disk.
        if let Some(workdir) = repo.workdir() {
            let _ = std::fs::remove_file(workdir.join(rel));
        }
        return Ok(());
    }
    // Tracked file: restore the working tree to the index (staged) state.
    let mut co = CheckoutBuilder::new();
    co.force().update_index(false).path(rel);
    repo.checkout_index(None, Some(&mut co))
}

/// Runs a `git` CLI subcommand in `root`, returning the combined output on success
/// or the error text on failure. Uses the CLI so it inherits the user's auth
/// (credential helpers, ssh-agent) exactly like their shell.
///
/// Never interactive: git runs under the TUI with no usable terminal, so a
/// username/passphrase prompt would hang the command (or scribble over the
/// screen). stdin is closed, `GIT_TERMINAL_PROMPT=0` turns HTTPS prompts into
/// errors, and ssh runs in batch mode — unless the user configured their own
/// ssh command (`GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand`), which is
/// left untouched.
fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(root)
        .args(args)
        .stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    let user_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || std::env::var_os("GIT_SSH").is_some()
        || Repository::discover(root)
            .and_then(|r| r.config())
            .and_then(|c| c.get_string("core.sshCommand"))
            .is_ok();
    if !user_ssh {
        cmd.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes");
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let msg = format!("{stdout}{stderr}").trim().to_string();
    if out.status.success() {
        Ok(msg)
    } else if msg.is_empty() {
        Err("git command failed".to_string())
    } else {
        Err(msg)
    }
}

/// `git fetch` — download remote refs (updates ahead/behind on the next status).
pub fn fetch(root: &Path) -> Result<String, String> {
    run_git(root, &["fetch"])
}

/// `git pull --ff-only` — fast-forward the current branch to its upstream.
pub fn pull(root: &Path) -> Result<String, String> {
    run_git(root, &["pull", "--ff-only"])
}

/// `git push` — publish local commits. Falls back to `-u origin HEAD` when the
/// branch has no upstream yet.
pub fn push(root: &Path) -> Result<String, String> {
    match run_git(root, &["push"]) {
        Err(e) if e.contains("upstream") || e.contains("set-upstream") => {
            run_git(root, &["push", "-u", "origin", "HEAD"])
        }
        other => other,
    }
}

/// Per-line marker in the editor gutter (working tree vs HEAD).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GutterKind {
    /// Added or modified line (green bar).
    Added,
    /// A deletion boundary sits right at this line (red wedge).
    Deleted,
}

/// The content of `path` at HEAD, or `None` if there is no repo / the file is not tracked.
pub fn head_file(path: &Path) -> Option<String> {
    let repo = Repository::discover(path.parent()?).ok()?;
    let workdir = repo.workdir()?.to_path_buf();
    let rel = path.strip_prefix(&workdir).ok()?;
    let tree = repo.head().ok()?.peel_to_tree().ok()?;
    let entry = tree.get_path(rel).ok()?;
    let blob = entry.to_object(&repo).ok()?.peel_to_blob().ok()?;
    Some(String::from_utf8_lossy(blob.content()).into_owned())
}

/// Per-line gutter markers for the difference between `old` (HEAD) and `new` (buffer).
/// Pure (no IO): computes the diff in memory, safe to call from `update`.
pub fn gutter_marks(old: &str, new: &str) -> Vec<(usize, GutterKind)> {
    let (old, new) = (eol_terminated(old), eol_terminated(new));
    let Some(patch) = line_patch(&old, &new) else {
        return Vec::new();
    };
    let mut marks: std::collections::HashMap<usize, GutterKind> = std::collections::HashMap::new();
    for h in 0..patch.num_hunks() {
        let Ok((hunk, _)) = patch.hunk(h) else {
            continue;
        };
        let new_start = hunk.new_start() as usize;
        let new_lines = hunk.new_lines() as usize;
        if new_lines > 0 {
            // Added / modified lines -> green. new_start is 1-based.
            for i in 0..new_lines {
                marks.insert(new_start.saturating_sub(1) + i, GutterKind::Added);
            }
        } else if hunk.old_lines() > 0 {
            // Pure deletion -> red wedge on the line before the removed block.
            let anchor = new_start.saturating_sub(1);
            marks.entry(anchor).or_insert(GutterKind::Deleted);
        }
    }
    marks.into_iter().collect()
}

/// Zero-context line diff of `old` -> `new` for the change gutter / inline diff.
/// Pass both sides through [`eol_terminated`] first.
fn line_patch<'a>(old: &'a str, new: &'a str) -> Option<git2::Patch<'a>> {
    let mut opts = git2::DiffOptions::new();
    opts.context_lines(0);
    git2::Patch::from_buffers(old.as_bytes(), None, new.as_bytes(), None, Some(&mut opts)).ok()
}

/// `s` with a trailing newline. The change gutter compares both sides this way:
/// otherwise deleting the last line (`"a\nb\nc"` -> `"a\nb"`) turns the line
/// above it from `"b\n"` into `"b"`, and that untouched line would show as
/// removed + re-added.
fn eol_terminated(s: &str) -> std::borrow::Cow<'_, str> {
    if s.is_empty() || s.ends_with('\n') {
        s.into()
    } else {
        format!("{s}\n").into()
    }
}

/// Removed line blocks for an inline diff view. Each entry is `(anchor, lines)`:
/// the removed `lines` render right after buffer line `anchor` (`None` = before
/// the first line). Includes the old side of modifications so replaced code is
/// shown alongside the new lines. Pure (no IO).
pub fn deleted_blocks(old: &str, new: &str) -> Vec<(Option<usize>, Vec<String>)> {
    let (old, new) = (eol_terminated(old), eol_terminated(new));
    let Some(patch) = line_patch(&old, &new) else {
        return Vec::new();
    };
    let old_lines: Vec<&str> = old.lines().collect();
    let mut out = Vec::new();
    for h in 0..patch.num_hunks() {
        let Ok((hunk, _)) = patch.hunk(h) else {
            continue;
        };
        let ol = hunk.old_lines() as usize;
        if ol == 0 {
            continue; // pure addition — nothing removed
        }
        let os = hunk.old_start() as usize; // 1-based
        let removed: Vec<String> = (0..ol)
            .filter_map(|i| old_lines.get(os - 1 + i).map(|s| s.to_string()))
            .collect();
        if removed.is_empty() {
            continue;
        }
        let nl = hunk.new_lines() as usize;
        let ns = hunk.new_start() as usize; // 1-based
        let anchor = if nl > 0 {
            // Render right before the first new (added/modified) line.
            let first_new0 = ns.saturating_sub(1); // 0-based first new line
            if first_new0 == 0 {
                None
            } else {
                Some(first_new0 - 1)
            }
        } else {
            // Pure deletion: right after the line the removal follows.
            if ns == 0 { None } else { Some(ns - 1) }
        };
        out.push((anchor, removed));
    }
    out
}

/// What one row of a commit's diff view is: it decides the row's gutter number
/// and how the editor styles it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommitRow {
    /// The author line at the top of the view.
    Author,
    /// The date / hash line under it.
    Meta,
    /// A line of the commit message.
    Message,
    /// A file heading ("model.rs  src/app/  +3 -1").
    File,
    /// The blank row that separates two hunks of the same file.
    Gap,
    /// A code line, carrying its line number in the commit's version of the file.
    Line(usize),
}

/// The two sides of a commit's changes, laid out for the inline diff view.
///
/// Both strings carry the *same* commit header, file headings and gaps, so those
/// rows diff as unchanged context and only the commit's real edits get a color:
/// `new` becomes the tab's buffer (added lines, green) and `old` its HEAD text
/// (removed lines, woven in red) — exactly how an uncommitted change is shown.
pub struct CommitDiff {
    /// The parent's side of every hunk.
    pub old: String,
    /// This commit's side of every hunk.
    pub new: String,
    /// What each line of `new` is (same length as its line count), so the editor
    /// can number the code rows with the file's own line numbers and style the
    /// heading rows.
    pub rows: Vec<CommitRow>,
    /// Extension of the file with the most changed lines, so the tab can pick a
    /// syntax to highlight the code with.
    pub syntax_ext: Option<String>,
}

/// Accumulates the two sides of the view line by line, keeping `rows` aligned
/// with the lines of `new`.
#[derive(Default)]
struct ViewBuilder {
    old: String,
    new: String,
    rows: Vec<CommitRow>,
}

impl ViewBuilder {
    /// A row present on both sides: unchanged context, so it stays uncolored.
    fn both(&mut self, text: &str, kind: CommitRow) {
        push_line(&mut self.old, text);
        push_line(&mut self.new, text);
        self.rows.push(kind);
    }

    /// A line the commit removed: only the parent has it (woven in red).
    fn removed(&mut self, text: &str) {
        push_line(&mut self.old, text);
    }

    /// A line the commit added: only this side has it (green), numbered `lineno`.
    fn added(&mut self, text: &str, lineno: usize) {
        push_line(&mut self.new, text);
        self.rows.push(CommitRow::Line(lineno));
    }
}

/// Builds the inline diff view of a commit against its first parent.
/// Blocking (git2) — call inside spawn_blocking.
pub fn commit_diff(root: &Path, hash: &str) -> Result<CommitDiff, String> {
    let repo = Repository::discover(root).map_err(|e| e.message().to_string())?;
    let commit = repo
        .revparse_single(hash)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| e.message().to_string())?;
    let tree = commit.tree().map_err(|e| e.message().to_string())?;
    // The root commit has no parent: diff against an empty tree so the whole
    // commit shows up as additions.
    let parent = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let diff = repo
        .diff_tree_to_tree(parent.as_ref(), Some(&tree), None)
        .map_err(|e| e.message().to_string())?;

    let mut v = ViewBuilder::default();

    // Who committed it, when, and the message — the header of the view.
    let author = commit.author();
    v.both(
        &format!(
            "{}  <{}>",
            author.name().unwrap_or(""),
            author.email().unwrap_or("")
        ),
        CommitRow::Author,
    );
    v.both(
        &format!("{}  ·  {}", format_time(commit.time()), commit.id()),
        CommitRow::Meta,
    );
    v.both("", CommitRow::Gap);
    for line in commit.message().unwrap_or("").trim_end().lines() {
        v.both(line, CommitRow::Message);
    }

    // The extension of the file with the most changed lines wins the syntax.
    let mut best: Option<(usize, String)> = None;

    for idx in 0..diff.deltas().len() {
        let Some(delta) = diff.get_delta(idx) else {
            continue;
        };
        let patch = git2::Patch::from_diff(&diff, idx).ok().flatten();
        let (added, removed) = patch
            .as_ref()
            .and_then(|p| p.line_stats().ok())
            .map(|(_, a, d)| (a, d))
            .unwrap_or((0, 0));

        v.both("", CommitRow::Gap);
        v.both(&file_heading(&delta, added, removed), CommitRow::File);

        let Some(patch) = patch else {
            // No patch: a binary file, or one git could not diff as text.
            v.both("    (binary file)", CommitRow::Message);
            continue;
        };
        if let Some(ext) = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            && best.as_ref().is_none_or(|(n, _)| added + removed > *n)
        {
            best = Some((added + removed, ext.to_string()));
        }

        for h in 0..patch.num_hunks() {
            let Ok((_, line_count)) = patch.hunk(h) else {
                continue;
            };
            // A blank row marks the lines skipped between two hunks; the jump in
            // the line numbers says how many.
            if h > 0 {
                v.both("", CommitRow::Gap);
            }
            for l in 0..line_count {
                let Ok(line) = patch.line_in_hunk(h, l) else {
                    continue;
                };
                let text = String::from_utf8_lossy(line.content());
                let text = text.strip_suffix('\n').unwrap_or(&text);
                match (line.origin(), line.new_lineno()) {
                    (' ', Some(n)) => v.both(text, CommitRow::Line(n as usize)),
                    ('+', Some(n)) => v.added(text, n as usize),
                    ('-', _) => v.removed(text),
                    // "\ No newline at end of file" and friends: not content.
                    _ => {}
                }
            }
        }
    }

    if v.old.is_empty() && v.new.is_empty() {
        return Err("empty commit".to_string());
    }
    Ok(CommitDiff {
        old: v.old,
        new: v.new,
        rows: v.rows,
        syntax_ext: best.map(|(_, ext)| ext),
    })
}

/// Appends `text` as one line, adding the line ending it may be missing (a
/// file's last line comes without one, which would glue the next row onto it).
fn push_line(out: &mut String, text: &str) {
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
}

/// The heading above a file's hunks: name, its directory, and the line counts —
/// "model.rs  src/app/  +3 -1".
fn file_heading(delta: &git2::DiffDelta, added: usize, removed: usize) -> String {
    let path = match delta.status() {
        git2::Delta::Deleted => delta.old_file().path(),
        _ => delta.new_file().path().or_else(|| delta.old_file().path()),
    };
    let path = path.unwrap_or(Path::new(""));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = match path.parent().map(|p| p.to_string_lossy().into_owned()) {
        Some(d) if !d.is_empty() => format!("{d}/"),
        _ => String::new(),
    };
    let what = match delta.status() {
        git2::Delta::Added => "(new file)",
        git2::Delta::Deleted => "(deleted)",
        git2::Delta::Renamed | git2::Delta::Copied => "(renamed)",
        _ => "",
    };
    // Two spaces between the parts, skipping the ones this file has nothing for
    // (a file in the repo root has no directory, an edit has no status word).
    [
        name.as_str(),
        dir.as_str(),
        what,
        &format!("+{added} -{removed}"),
    ]
    .iter()
    .filter(|p| !p.is_empty())
    .cloned()
    .collect::<Vec<&str>>()
    .join("  ")
}

/// `git2::Time` as "YYYY-MM-DD HH:MM:SS +ZZZZ" in the commit's own timezone.
fn format_time(t: git2::Time) -> String {
    let offset_min = t.offset_minutes() as i64;
    let local = t.seconds() + offset_min * 60;
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let secs = local.rem_euclid(86_400);
    let (hh, mm, ss) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let sign = if offset_min < 0 { '-' } else { '+' };
    let off = offset_min.abs();
    format!(
        "{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02} {sign}{:02}{:02}",
        off / 60,
        off % 60
    )
}

/// Days since the Unix epoch -> (year, month, day). Howard Hinnant's
/// `civil_from_days`, so no date crate is needed for the one timestamp we print.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Commits the changes in the index.
pub fn commit(root: &Path, message: &str) -> Result<(), git2::Error> {
    let repo = Repository::discover(root)?;
    let sig = repo.signature()?;
    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let parents: Vec<git2::Commit> = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .into_iter()
        .collect();
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)?;
    Ok(())
}

/// Undoes the last commit (`git reset --soft HEAD~1`): moves HEAD to the parent
/// while leaving the index and worktree untouched, so the commit's changes stay
/// staged. Returns the message of the undone commit so the UI can repopulate the
/// commit box. Fails on the initial (parentless) commit.
pub fn undo_last_commit(root: &Path) -> Result<String, git2::Error> {
    let repo = Repository::discover(root)?;
    let head = repo.head()?.peel_to_commit()?;
    let message = head.message().unwrap_or("").to_string();
    let parent = head.parent(0)?; // errors on the root commit (no parent)
    repo.reset(parent.as_object(), git2::ResetType::Soft, None)?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(old: &str, new: &str) -> Vec<(usize, GutterKind)> {
        let mut v = gutter_marks(old, new);
        v.sort_by_key(|(l, _)| *l);
        v
    }

    #[test]
    fn added_lines_are_green() {
        // Insert a new line after line 1.
        let m = kinds("a\nb\n", "a\nx\nb\n");
        assert_eq!(m, vec![(1, GutterKind::Added)]);
    }

    #[test]
    fn modified_line_is_green() {
        let m = kinds("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(m, vec![(1, GutterKind::Added)]);
    }

    #[test]
    fn pure_deletion_marks_boundary_red() {
        // Delete line 2 ("b"); wedge anchors on the line before it (index 0).
        let m = kinds("a\nb\nc\n", "a\nc\n");
        assert_eq!(m, vec![(0, GutterKind::Deleted)]);
    }

    #[test]
    fn no_changes_no_marks() {
        assert!(kinds("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn deleted_block_after_anchor_line() {
        // Remove "b" -> shown right after line 0 ("a").
        let d = deleted_blocks("a\nb\nc\n", "a\nc\n");
        assert_eq!(d, vec![(Some(0), vec!["b".to_string()])]);
    }

    #[test]
    fn modification_shows_old_line() {
        // "b" -> "B": the old "b" is a removed row before the new "B".
        let d = deleted_blocks("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(d, vec![(Some(0), vec!["b".to_string()])]);
    }

    #[test]
    fn deleting_the_last_line_leaves_the_line_above_unchanged() {
        // Without a trailing newline on one side, "b\n" vs "b" used to diff as
        // a modification, painting the untouched "b" as deleted too.
        for old in ["a\nb\nc\n", "a\nb\nc"] {
            assert_eq!(kinds(old, "a\nb"), vec![(1, GutterKind::Deleted)]);
            assert_eq!(
                deleted_blocks(old, "a\nb"),
                vec![(Some(1), vec!["c".to_string()])]
            );
        }
    }

    #[test]
    fn deletion_at_top_anchors_before_first_line() {
        let d = deleted_blocks("a\nb\n", "b\n");
        assert_eq!(d, vec![(None, vec!["a".to_string()])]);
    }

    #[test]
    fn pure_addition_has_no_deleted_blocks() {
        assert!(deleted_blocks("a\nb\n", "a\nx\nb\n").is_empty());
    }

    #[test]
    fn commit_diff_sides_share_context_and_split_changes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let Ok(repo) = Repository::discover(root) else {
            return; // not a checkout (packaged source)
        };
        let Some(head) = repo.head().ok().and_then(|h| h.target()) else {
            return;
        };
        let d = commit_diff(root, &head.to_string()).unwrap();
        // Both sides open with the same author/date header (unchanged context).
        for side in [&d.old, &d.new] {
            let mut head_lines = side.lines();
            assert!(head_lines.next().unwrap().contains('@')); // author <email>
            assert!(head_lines.next().unwrap().contains('·')); // date · full hash
        }
        // One row label per line of the view, so the gutter never runs off the end.
        assert_eq!(d.rows.len(), d.new.lines().count());
        // The code rows are numbered by the file, not by the view: the first one
        // is a real line number and they only ever move forward within a file.
        let numbers: Vec<usize> = d
            .rows
            .iter()
            .filter_map(|r| match r {
                CommitRow::Line(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert!(!numbers.is_empty());
        assert!(numbers.iter().all(|n| *n >= 1));
        // Every heading row names a file and its line counts.
        let headings: Vec<&str> = d
            .rows
            .iter()
            .zip(d.new.lines())
            .filter(|(r, _)| **r == CommitRow::File)
            .map(|(_, l)| l)
            .collect();
        assert!(!headings.is_empty());
        assert!(
            headings
                .iter()
                .all(|h| h.contains(" +") && h.contains(" -"))
        );
        // The two sides differ only where the commit actually changed something,
        // which is what paints the green/red backgrounds.
        assert_ne!(d.old, d.new);
        assert!(!gutter_marks(&d.old, &d.new).is_empty());
        // Every line is terminated, so no two rows can run together.
        assert!(d.old.ends_with('\n') && d.new.ends_with('\n'));
    }

    #[test]
    fn commit_diff_rejects_an_unknown_revision() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        if Repository::discover(root).is_err() {
            return;
        }
        assert!(commit_diff(root, "0000000").is_err());
    }

    #[test]
    fn epoch_and_offset_format() {
        assert_eq!(
            format_time(git2::Time::new(0, 0)),
            "1970-01-01 00:00:00 +0000"
        );
        // +03:00 shifts the wall clock forward by three hours.
        assert_eq!(
            format_time(git2::Time::new(1_700_000_000, 180)),
            "2023-11-15 01:13:20 +0300"
        );
    }
}
