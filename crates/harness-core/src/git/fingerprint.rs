//! What the changed files of the folder look like right now, so the harness
//! can tell later whether anyone touched them since.

use super::{GitError, Repo};

/// One changed file: its path and git's hash of its contents, or `None` when
/// the file is deleted.
pub type FileFingerprint = (String, Option<String>);

/// How many files one `git hash-object` call hashes. Starting git once per
/// file is slow on Windows; one call for hundreds of paths could pass the
/// command-line length limit there (about 32 000 characters).
const HASH_BATCH: usize = 100;

/// What stands in for the contents of a changed folder (a nested repository):
/// only regular files have contents to hash.
const FOLDER: &str = "folder";

impl Repo {
    /// Every changed file outside `.harness/` with the hash of its contents,
    /// sorted by path. Two equal lists mean the same changes.
    ///
    /// Not seen: changes inside a nested repository (the folder counts as one
    /// entry) and file permissions.
    ///
    /// # Errors
    ///
    /// When git cannot read the folder or a file.
    pub fn changes_fingerprint(&self) -> Result<Vec<FileFingerprint>, GitError> {
        let mut list: Vec<FileFingerprint> = Vec::new();
        // Regular files, and where each one is in `list`, to hash them together.
        let mut files: Vec<String> = Vec::new();
        let mut slots: Vec<usize> = Vec::new();
        for path in self.changed_files()? {
            if is_harness(&path) {
                continue;
            }
            let full = self.root().join(&path);
            if full.is_file() {
                files.push(path.clone());
                slots.push(list.len());
                list.push((path, None)); // the hash comes below
            } else if full.exists() {
                list.push((path, Some(FOLDER.to_string())));
            } else {
                list.push((path, None));
            }
        }
        for (slot, hash) in slots.into_iter().zip(self.hash_files(&files)?) {
            list[slot].1 = Some(hash);
        }
        Ok(list)
    }

    /// Git's hash of each file's contents, in the order given.
    fn hash_files(&self, files: &[String]) -> Result<Vec<String>, GitError> {
        let mut hashes = Vec::with_capacity(files.len());
        for batch in files.chunks(HASH_BATCH) {
            let mut args = vec!["hash-object", "--no-filters", "--"];
            args.extend(batch.iter().map(String::as_str));
            let out = self.git_literal(&args)?;
            hashes.extend(out.lines().map(|line| line.trim().to_string()));
        }
        Ok(hashes)
    }
}

/// Is this path the harness's own folder or inside it?
pub(crate) fn is_harness(path: &str) -> bool {
    path == ".harness" || path.starts_with(".harness/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn the_fingerprint_changes_with_the_contents() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        fs::write(dir.path().join("gone.txt"), "x\n").unwrap();
        repo.commit_all("first").unwrap();
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        fs::write(dir.path().join("b.txt"), "two\n").unwrap();
        fs::remove_file(dir.path().join("gone.txt")).unwrap();
        fs::create_dir_all(dir.path().join(".harness")).unwrap();
        fs::write(dir.path().join(".harness/log.txt"), "log\n").unwrap();

        let first = repo.changes_fingerprint().unwrap();
        assert_eq!(first.len(), 3, "{first:?}");
        assert_eq!(first[0].1, first[1].1, "same contents, same hash");
        assert!(first[0].1.is_some());
        assert_eq!(first[2], ("gone.txt".to_string(), None));
        assert_eq!(repo.changes_fingerprint().unwrap(), first);

        fs::write(dir.path().join("a.txt"), "three\n").unwrap();
        assert_ne!(repo.changes_fingerprint().unwrap(), first);
    }

    #[test]
    fn many_files_are_hashed_in_batches() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        for n in 0..(HASH_BATCH + 5) {
            fs::write(dir.path().join(format!("f{n:03}.txt")), format!("{n}\n")).unwrap();
        }

        let list = repo.changes_fingerprint().unwrap();

        assert_eq!(list.len(), HASH_BATCH + 5);
        let (path, hash) = list.last().unwrap();
        let expected = repo
            .git_literal(&["hash-object", "--no-filters", "--", path])
            .unwrap();
        assert_eq!(hash.as_deref(), Some(expected.trim()));
    }
}
