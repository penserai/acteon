//! Bounded subprocess collection with cancellation and process-group cleanup.

use std::io;
use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

pub(crate) const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Every managed subprocess owns a new process group on Unix.
pub(crate) fn configure(command: &mut Command) -> &mut Command {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    command
}

struct ProcessGroup(Option<u32>);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.and_then(|pid| i32::try_from(pid).ok()) {
            // The group is created by `configure`, never inherited from the host.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

async fn read_bounded(reader: Option<impl AsyncRead + Unpin>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(reader) = reader {
        reader
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > MAX_OUTPUT_BYTES {
            return Err(io::Error::other("subprocess output exceeded 1 MiB limit"));
        }
    }
    Ok(bytes)
}

pub(crate) async fn wait(mut child: Child, timeout: Duration) -> io::Result<Output> {
    let group = ProcessGroup(child.id());
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let result = tokio::time::timeout(timeout, async {
        let (status, stdout, stderr) =
            tokio::try_join!(child.wait(), read_bounded(stdout), read_bounded(stderr))?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    })
    .await;
    // Also terminate background descendants after a successful parent exit.
    drop(group);
    match result {
        Ok(Ok(output)) => Ok(output),
        other => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            match other {
                Ok(Err(error)) => Err(error),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "subprocess timed out",
                )),
                Ok(Ok(_)) => unreachable!(),
            }
        }
    }
}

pub(crate) async fn run(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let child = configure(command).spawn()?;
    wait(child, timeout).await
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timeout_kills_descendants_before_returning() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command
            .args(["-c", "(sleep 0.2; touch late) & wait"])
            .current_dir(dir.path());
        let error = run(&mut command, Duration::from_millis(30))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!dir.path().join("late").exists());
    }

    #[tokio::test]
    async fn output_limit_terminates_writer() {
        let error = run(&mut Command::new("yes"), Duration::from_secs(3))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("output exceeded"));
    }

    #[tokio::test]
    async fn cancellation_kills_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command
            .args(["-c", "(sleep 0.2; touch late) & wait"])
            .current_dir(dir.path());
        let child = configure(&mut command).spawn().unwrap();
        let task = tokio::spawn(wait(child, Duration::from_secs(5)));
        tokio::time::sleep(Duration::from_millis(30)).await;
        task.abort();
        let _ = task.await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!dir.path().join("late").exists());
    }
}
