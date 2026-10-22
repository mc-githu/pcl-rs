//! Spawning the game and pumping its output.
//!
//! The process starts with `CREATE_NO_WINDOW` so no console flashes up, while
//! `java.exe` (not `javaw.exe`) is used so stdout/stderr can be captured into the
//! log file the crash analyser reads.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::args::CommandSpec;

/// Receives every line of game output (used by the GUI log view).
pub type LineSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct GameLog {
    pub path: PathBuf,
}

pub struct Launcher {
    log_dir: PathBuf,
}

impl Launcher {
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        Self {
            log_dir: log_dir.into(),
        }
    }

    pub async fn launch(
        &self,
        spec: &CommandSpec,
        env: &[(&str, String)],
        sink: Option<LineSink>,
    ) -> Result<GameHandle> {
        tokio::fs::create_dir_all(&self.log_dir)
            .await
            .with_context(|| format!("creating log dir {}", self.log_dir.display()))?;

        let log_path = self.log_dir.join(format!("{}-game.log", timestamp()));
        let log_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .with_context(|| format!("opening log {}", log_path.display()))?;

        tracing::info!("launching: {}", spec.display());

        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in env {
            command.env(key, value);
        }

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", spec.program.display()))?;

        let pid = child.id().unwrap_or(0);
        let shared_log = Arc::new(tokio::sync::Mutex::new(log_file));

        {
            let mut file = shared_log.lock().await;
            let header = format!(
                "=== PCL-RS launch {} ===\ncwd: {}\ncmd: {}\n\n",
                timestamp(),
                spec.cwd.display(),
                spec.display()
            );
            let _ = file.write_all(header.as_bytes()).await;
            let _ = file.flush().await;
        }

        let mut pumps = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            pumps.push(spawn_pump(stdout, shared_log.clone(), sink.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            pumps.push(spawn_pump(stderr, shared_log.clone(), sink));
        }

        Ok(GameHandle {
            pid,
            log: GameLog { path: log_path },
            child,
            pumps,
        })
    }
}

fn spawn_pump<R>(
    reader: R,
    log: Arc<tokio::sync::Mutex<tokio::fs::File>>,
    sink: Option<LineSink>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    {
                        let mut file = log.lock().await;
                        let _ = file.write_all(line.as_bytes()).await;
                        let _ = file.write_all(b"\n").await;
                        let _ = file.flush().await;
                    }
                    if let Some(sink) = &sink {
                        sink(&line);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!("game output pump stopped: {e}");
                    break;
                }
            }
        }
    })
}

pub struct GameHandle {
    pub pid: u32,
    pub log: GameLog,
    child: tokio::process::Child,
    pumps: Vec<tokio::task::JoinHandle<()>>,
}

impl GameHandle {
    /// Wait for the game to exit, then drain the output pumps.
    pub async fn wait(mut self) -> Result<i32> {
        let status = self.child.wait().await.context("waiting for the game")?;
        for pump in self.pumps.drain(..) {
            let _ = pump.await;
        }
        Ok(status.code().unwrap_or(-1))
    }

    /// Ask the game to exit (used by "stop" in the UI).
    pub async fn kill(&mut self) -> Result<()> {
        self.child.kill().await.context("killing the game process")
    }
}

/// `YYYYMMDD-HHMMSS` (UTC), enough to sort log files.
pub fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm (no chrono dependency).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_has_expected_shape() {
        let ts = timestamp();
        assert_eq!(ts.len(), 15, "{ts}");
        assert_eq!(&ts[8..9], "-");
    }

    #[test]
    fn civil_dates_are_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}
