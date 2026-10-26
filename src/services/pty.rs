//! A real shell PTY session via portable-pty.

use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::Result;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::msg::Msg;

/// Output the reader thread has buffered but the UI hasn't consumed yet.
/// Past this the reader stops reading, so a flood (`yes`, `cat big.log`)
/// back-pressures the shell instead of growing memory without bound.
const MAX_PENDING_OUTPUT: usize = 4 * 1024 * 1024;

/// Shell output shared between the reader thread and the session. The reader
/// appends and sends one wake-up `Msg::PtyOutput` per batch; the UI drains
/// everything accumulated since with [`PtySession::take_output`], so a burst
/// of reads costs one message and one vt100 pass, not one per 8 KiB read.
#[derive(Default)]
struct OutputQueue {
    state: Mutex<OutputState>,
    /// Signalled when the UI drains, to wake a reader waiting on the cap.
    drained: Condvar,
}

#[derive(Default)]
struct OutputState {
    buf: Vec<u8>,
    /// A wake-up message is in flight and hasn't been answered by a drain.
    notified: bool,
}

/// An open PTY session; held inside the Model.
pub struct PtySession {
    pub writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    output: Arc<OutputQueue>,
}

/// Closing a terminal tab drops its session: end the shell (and whatever it
/// runs in the foreground) instead of leaving it behind.
impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl PtySession {
    /// Takes every byte the shell produced since the last call. Call on each
    /// `Msg::PtyOutput` (a wake-up; it carries only the terminal id) and once
    /// right after storing the session from `Msg::PtyReady`, since output may
    /// have been announced before the session reached the Model.
    pub fn take_output(&self) -> Vec<u8> {
        let mut st = self.output.state.lock().unwrap_or_else(|e| e.into_inner());
        st.notified = false;
        let out = std::mem::take(&mut st.buf);
        drop(st);
        self.output.drained.notify_all();
        out
    }

    pub fn write(&mut self, data: &[u8]) {
        let _ = self.writer.write_all(data);
        let _ = self.writer.flush();
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

/// The user's shell: `$SHELL`, else `/bin/sh`.
fn shell_path() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

/// File name of the user's shell (`bash`), for terminal tab labels.
pub fn shell_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let path = shell_path();
        std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(path)
    })
}

/// Opens a new PTY, starts the user's shell, and sets up the reader thread.
/// Output is buffered in the session (see [`PtySession::take_output`]) and
/// announced with an empty `Msg::PtyOutput(id)`; closure is `Msg::PtyExited(id)`.
/// `id` names the terminal tab the session belongs to.
pub fn spawn(
    id: u64,
    cwd: std::path::PathBuf,
    rows: u16,
    cols: u16,
    tx: UnboundedSender<Msg>,
) -> Result<PtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(shell_path());
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");

    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    let output = Arc::new(OutputQueue::default());
    let queue = Arc::clone(&output);

    // Blocking read on a separate thread; buffers bytes into the shared queue.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if !push_output(&queue, &buf[..n], id, &tx) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(Msg::PtyExited(id));
    });

    Ok(PtySession {
        writer,
        master: pair.master,
        child,
        output,
    })
}

/// Appends a read to the queue, waiting while the UI is [`MAX_PENDING_OUTPUT`]
/// behind, and sends a wake-up unless one is already pending. `false` once
/// the session is gone (dropped, or the channel closed): the reader stops.
fn push_output(queue: &Arc<OutputQueue>, bytes: &[u8], id: u64, tx: &UnboundedSender<Msg>) -> bool {
    let mut st = queue.state.lock().unwrap_or_else(|e| e.into_inner());
    while st.buf.len() >= MAX_PENDING_OUTPUT {
        // Only this thread holds the other reference once the session drops.
        if Arc::strong_count(queue) == 1 {
            return false;
        }
        st = queue
            .drained
            .wait_timeout(st, Duration::from_millis(100))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    st.buf.extend_from_slice(bytes);
    let wake = !st.notified;
    st.notified = true;
    drop(st);
    !wake || tx.send(Msg::PtyOutput(id)).is_ok()
}
