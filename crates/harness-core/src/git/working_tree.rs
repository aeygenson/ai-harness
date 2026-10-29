//! The uncommitted changes in the project folder: saving them as a patch and
//! throwing them away after a failed attempt of a role.

use super::{GitError, Repo};

/// A git path rule for «everything except `.harness/`», the harness's own files.
const OUTSIDE_HARNESS: &str = ":(exclude).harness";

impl Repo {
    /// Every change outside `.harness/` as one patch that `git apply` puts
    /// back: changed, deleted and new files, binary ones too. Ignored files
    /// are left out. Empty when nothing changed or there is no commit yet.
    pub fn changes_as_patch(&self) -> Result<Vec<u8>, GitError> {
        if self.head()?.is_none() {
            return Ok(Vec::new());
        }
        // New files are in no diff until they are staged, so everything is
        // staged for the diff and unstaged again afterwards (the files
        // themselves stay as they are).
        self.git(&["add", "-A", "--", ".", OUTSIDE_HARNESS])?;
        // The user's own git settings must not change the patch: no other
        // diff program, no converted text, no colours, and the usual `a/` and
        // `b/` before the paths, which `git apply` expects.
        let patch = self.run_bytes(
            &[
                "diff",
                "--cached",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "HEAD",
                "--",
                ".",
                OUTSIDE_HARNESS,
            ],
            &[],
        );
        self.git(&["reset", "-q"])?;
        patch
    }

    /// Throws away every change outside `.harness/`, so a failed attempt of a role
    /// leaves nothing behind. Ignored files (like `target/`) are kept.
    pub fn discard_changes(&self) -> Result<(), GitError> {
        if self.head()?.is_none() {
            return Ok(());
        }
        // 1. Unstage everything the agent may have `git add`-ed.
        self.git(&["reset", "-q"])?;
        // 2. Put changed and deleted tracked files back as they are in the last commit.
        let out = self.git(&[
            "diff",
            "--name-only",
            "-z",
            "HEAD",
            "--",
            ".",
            OUTSIDE_HARNESS,
        ])?;
        let tracked: Vec<&str> = out.split('\0').filter(|p| !p.is_empty()).collect();
        if !tracked.is_empty() {
            let mut checkout = vec!["checkout", "-q", "HEAD", "--"];
            checkout.extend(&tracked);
            self.git_literal(&checkout)?;
        }
        // 3. Delete new files and folders.
        self.git(&["clean", "-fdq", "--", ".", OUTSIDE_HARNESS])?;
        Ok(())
    }
}
