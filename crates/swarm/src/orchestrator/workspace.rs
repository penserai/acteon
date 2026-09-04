//! Isolated recovery candidates. The original index and stash are never mutated.

use std::io::Write as _;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::error::SwarmError;

fn git(repo: &Path, args: &[&str]) -> Result<Output, SwarmError> {
    checked(Command::new("git").current_dir(repo).args(args))
}

fn checked(command: &mut Command) -> Result<Output, SwarmError> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(SwarmError::EvalHarness(format!(
            "git operation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output)
}

fn text(output: Output) -> Result<String, SwarmError> {
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|e| SwarmError::EvalHarness(e.to_string()))
}

/// Capture tracked and nonignored untracked files through a private Git index.
fn capture_tree(repo: &Path) -> Result<String, SwarmError> {
    let index_dir = tempfile::tempdir()?;
    let index = index_dir.path().join("index");
    let current_index = PathBuf::from(text(git(repo, &["rev-parse", "--git-path", "index"])?)?);
    let current_index = if current_index.is_absolute() {
        current_index
    } else {
        repo.join(current_index)
    };
    if current_index.exists() {
        std::fs::copy(current_index, &index)?;
    } else {
        checked(
            Command::new("git")
                .current_dir(repo)
                .env("GIT_INDEX_FILE", &index)
                .args(["read-tree", "HEAD"]),
        )?;
    }
    checked(
        Command::new("git")
            .current_dir(repo)
            .env("GIT_INDEX_FILE", &index)
            .args(["add", "--all"]),
    )?;
    text(checked(
        Command::new("git")
            .current_dir(repo)
            .env("GIT_INDEX_FILE", &index)
            .arg("write-tree"),
    )?)
}

/// A disposable candidate based on the current working tree, including dirty files.
pub struct WorkspaceCandidate {
    original: PathBuf,
    directory: tempfile::TempDir,
    baseline: String,
}

impl WorkspaceCandidate {
    /// Create an immutable baseline commit and detached candidate worktree.
    pub fn create(original: &Path) -> Result<Self, SwarmError> {
        let original = original.canonicalize()?;
        let root = text(git(&original, &["rev-parse", "--show-toplevel"])?)?;
        if Path::new(&root).canonicalize()? != original {
            return Err(SwarmError::EvalHarness(
                "recovery working directory must be the Git repository root".into(),
            ));
        }
        let baseline = capture_tree(&original)?;
        let parent = text(git(&original, &["rev-parse", "HEAD"])?)?;
        let commit = text(checked(
            Command::new("git")
                .current_dir(&original)
                .env("GIT_AUTHOR_NAME", "Acteon recovery")
                .env("GIT_AUTHOR_EMAIL", "recovery@localhost")
                .env("GIT_COMMITTER_NAME", "Acteon recovery")
                .env("GIT_COMMITTER_EMAIL", "recovery@localhost")
                .args([
                    "commit-tree",
                    &baseline,
                    "-p",
                    &parent,
                    "-m",
                    "Isolated recovery baseline",
                ]),
        )?)?;
        let directory = tempfile::tempdir()?;
        checked(
            Command::new("git")
                .current_dir(&original)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "worktree",
                    "add",
                    "--detach",
                ])
                .arg(directory.path().join("candidate"))
                .arg(&commit),
        )?;
        Ok(Self {
            original,
            directory,
            baseline,
        })
    }

    /// Directory in which recovery and evaluation execute.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.directory.path().join("candidate")
    }

    /// Promote a verified candidate only if the original working tree is unchanged.
    pub fn promote(&self) -> Result<(), SwarmError> {
        if capture_tree(&self.original)? != self.baseline {
            return Err(SwarmError::EvalHarness(
                "original workspace changed during recovery; candidate was not applied".into(),
            ));
        }
        let candidate_tree = capture_tree(&self.path())?;
        let patch = git(
            &self.original,
            &[
                "diff",
                "--binary",
                "--no-ext-diff",
                &self.baseline,
                &candidate_tree,
            ],
        )?
        .stdout;
        if patch.is_empty() {
            return Ok(());
        }
        // Git applies the complete patch transactionally, without staging changes.
        let mut child = Command::new("git")
            .current_dir(&self.original)
            .args(["apply", "--binary", "--whitespace=nowarn", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| SwarmError::EvalHarness("missing git stdin".into()))?
            .write_all(&patch)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(SwarmError::EvalHarness(format!(
                "candidate promotion failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
}

impl Drop for WorkspaceCandidate {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .current_dir(&self.original)
            .args(["worktree", "remove", "--force"])
            .arg(self.path())
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]).unwrap();
        git(dir.path(), &["config", "user.name", "test"]).unwrap();
        git(dir.path(), &["config", "user.email", "test@localhost"]).unwrap();
        std::fs::write(dir.path().join("tracked"), "base\n").unwrap();
        git(dir.path(), &["add", "."]).unwrap();
        git(dir.path(), &["commit", "-qm", "base"]).unwrap();
        dir
    }

    #[test]
    fn rejected_candidate_preserves_dirty_index_untracked_and_stash() {
        let dir = repo();
        std::fs::write(dir.path().join("tracked"), "stash\n").unwrap();
        git(dir.path(), &["stash", "push", "-qm", "user stash"]).unwrap();
        std::fs::write(dir.path().join("tracked"), "staged\n").unwrap();
        git(dir.path(), &["add", "tracked"]).unwrap();
        std::fs::write(dir.path().join("tracked"), "primary work\n").unwrap();
        std::fs::write(dir.path().join("new file"), "untracked\n").unwrap();
        let status = git(dir.path(), &["status", "--porcelain=v1"])
            .unwrap()
            .stdout;
        let staged = git(dir.path(), &["diff", "--cached"]).unwrap().stdout;
        let stash = git(dir.path(), &["stash", "list"]).unwrap().stdout;
        {
            let candidate = WorkspaceCandidate::create(dir.path()).unwrap();
            assert_eq!(
                std::fs::read_to_string(candidate.path().join("tracked")).unwrap(),
                "primary work\n"
            );
            assert!(candidate.path().join("new file").exists());
            std::fs::write(candidate.path().join("tracked"), "rejected\n").unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tracked")).unwrap(),
            "primary work\n"
        );
        assert_eq!(
            git(dir.path(), &["status", "--porcelain=v1"])
                .unwrap()
                .stdout,
            status
        );
        assert_eq!(
            git(dir.path(), &["diff", "--cached"]).unwrap().stdout,
            staged
        );
        assert_eq!(git(dir.path(), &["stash", "list"]).unwrap().stdout, stash);
    }

    #[test]
    fn verified_candidate_promotes_new_and_modified_files() {
        let dir = repo();
        let candidate = WorkspaceCandidate::create(dir.path()).unwrap();
        std::fs::write(candidate.path().join("tracked"), "fixed\n").unwrap();
        std::fs::write(candidate.path().join("new"), "new\n").unwrap();
        candidate.promote().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tracked")).unwrap(),
            "fixed\n"
        );
        assert!(dir.path().join("new").exists());
    }

    #[test]
    fn concurrent_original_edit_prevents_promotion() {
        let dir = repo();
        let candidate = WorkspaceCandidate::create(dir.path()).unwrap();
        std::fs::write(candidate.path().join("tracked"), "candidate\n").unwrap();
        std::fs::write(dir.path().join("tracked"), "user edit\n").unwrap();
        assert!(candidate.promote().is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tracked")).unwrap(),
            "user edit\n"
        );
    }
}
