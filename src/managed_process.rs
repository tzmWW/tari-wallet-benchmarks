use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::Context;
use tokio::{process::Command, time};

pub struct ManagedProcess {
    label: String,
    child: tokio::process::Child,
    process_group_id: Option<u32>,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

impl ManagedProcess {
    pub fn spawn(label: &str, mut command: Command, log_dir: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(log_dir)?;
        let stdout_path = log_dir.join(format!("{label}.stdout.log"));
        let stderr_path = log_dir.join(format!("{label}.stderr.log"));
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stdout_path)?;
        let stderr = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_path)?;
        #[cfg(unix)]
        command.process_group(0);
        let child = command
            .kill_on_drop(true)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("spawning {label}"))?;
        let process_group_id = child.id();
        Ok(Self {
            label: label.to_string(),
            child,
            process_group_id,
            stdout_path,
            stderr_path,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> anyhow::Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .with_context(|| format!("checking {} process status", self.label))
    }

    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.try_wait()?.is_some() {
            return self.ensure_descendants_stopped().await;
        }
        signal_group(self.process_group_id, graceful_signal());
        if time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            signal_group(self.process_group_id, force_signal());
            time::timeout(Duration::from_secs(5), self.child.wait())
                .await
                .with_context(|| format!("waiting for {} after forced shutdown", self.label))??;
        }
        self.ensure_descendants_stopped().await?;
        Ok(())
    }

    pub async fn wait(&mut self, timeout: Duration) -> anyhow::Result<ExitStatus> {
        let status = time::timeout(timeout, self.child.wait())
            .await
            .with_context(|| format!("waiting for {} to exit", self.label))??;
        self.ensure_descendants_stopped().await?;
        Ok(status)
    }

    async fn ensure_descendants_stopped(&self) -> anyhow::Result<()> {
        if !group_exists(self.process_group_id) {
            return Ok(());
        }
        signal_group(self.process_group_id, graceful_signal());
        let deadline = time::Instant::now() + Duration::from_secs(2);
        while group_exists(self.process_group_id) && time::Instant::now() < deadline {
            time::sleep(Duration::from_millis(25)).await;
        }
        if group_exists(self.process_group_id) {
            signal_group(self.process_group_id, force_signal());
        }
        let deadline = time::Instant::now() + Duration::from_secs(2);
        while group_exists(self.process_group_id) && time::Instant::now() < deadline {
            time::sleep(Duration::from_millis(25)).await;
        }
        if group_exists(self.process_group_id) {
            anyhow::bail!("{} process group still has live descendants", self.label);
        }
        Ok(())
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        let mut leader_exited = self.child.try_wait().ok().flatten().is_some();
        if leader_exited && !group_exists(self.process_group_id) {
            return;
        }
        signal_group(self.process_group_id, graceful_signal());
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            leader_exited |= self.child.try_wait().ok().flatten().is_some();
            if leader_exited && !group_exists(self.process_group_id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        signal_group(self.process_group_id, force_signal());
        let _ = self.child.start_kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            leader_exited |= self.child.try_wait().ok().flatten().is_some();
            if leader_exited && !group_exists(self.process_group_id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        signal_group(self.process_group_id, force_signal());
    }
}

#[cfg(unix)]
fn signal_group(pid: Option<u32>, signal: i32) {
    if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
        // SAFETY: a negative PID targets the subprocess group created at spawn.
        unsafe {
            libc::kill(-pid, signal);
        }
    }
}

#[cfg(unix)]
fn group_exists(pid: Option<u32>) -> bool {
    pid.and_then(|pid| i32::try_from(pid).ok())
        .is_some_and(|pid| {
            // SAFETY: signal 0 checks whether any process remains in the group.
            unsafe { libc::kill(-pid, 0) == 0 }
        })
}

#[cfg(not(unix))]
fn group_exists(_: Option<u32>) -> bool {
    false
}

#[cfg(not(unix))]
fn signal_group(_: Option<u32>, _: i32) {}

#[cfg(unix)]
const fn graceful_signal() -> i32 {
    libc::SIGTERM
}

#[cfg(not(unix))]
const fn graceful_signal() -> i32 {
    0
}

#[cfg(unix)]
const fn force_signal() -> i32 {
    libc::SIGKILL
}

#[cfg(not(unix))]
const fn force_signal() -> i32 {
    0
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_reaps_the_managed_process_group() {
        let logs = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        let mut process = ManagedProcess::spawn("group-test", command, logs.path()).unwrap();
        assert!(process.id().is_some());
        process.shutdown().await.unwrap();
        assert!(process.try_wait().unwrap().is_some());
    }
}
