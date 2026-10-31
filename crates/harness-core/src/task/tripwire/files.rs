//! Reading one watched file or folder into a [`Content`], writing it back,
//! and giving the folders their access rights back.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A file bigger than this is remembered by its size and the time it was
/// last changed instead of its whole content: the `harness` program is tens
/// of MB, and reading it twice per role would be slow. (A program that also
/// forges the time could hide a change of such a file.) It can be reported,
/// but not put back.
pub(super) const MAX_KEPT_BYTES: u64 = 1024 * 1024;

/// What a file held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Content {
    /// The whole content of a small file, and its permissions: git runs a
    /// hook only when it is executable, so a hook put back without them
    /// would be silently skipped.
    File {
        bytes: Vec<u8>,
        permissions: fs::Permissions,
    },
    /// A folder and its access rights ([`harness_platform::file::folder_mode`]).
    /// It is watched on its own: an empty folder in place of a hook, or a
    /// folder that others may now write to, is a change too.
    Folder(u32),
    /// A symbolic link, and where it points (not followed: the file it
    /// points to may be anywhere and is not watched).
    Link(PathBuf),
    /// A big file's size and the time it was last changed.
    Summary {
        len: u64,
        modified: Option<SystemTime>,
    },
    /// Something that is not a plain file, a link or a folder: a named pipe
    /// (FIFO), a socket or a device. It is never opened: reading a pipe
    /// waits until someone writes to it, which could be forever.
    Special,
    /// A file that exists but cannot be read (for example no permission);
    /// this kind of error is kept, so a change of it is noticed too.
    Unreadable(io::ErrorKind),
}

/// Adds `path` to `files`: a folder as one entry and every file and folder
/// in it (at any depth), anything else (a file, a link) as one entry. A file
/// or folder that cannot be read is an [`Content::Unreadable`] entry, so one
/// such file, maybe planted on purpose, does not stop the check of all the
/// others.
///
/// Each folder's permissions are noted *before* the folder is unlocked for
/// reading, and it is left unlocked: [`put_back_folder_modes`] gives the
/// folders their noted permissions back.
pub(super) fn add_tree(path: &Path, files: &mut BTreeMap<PathBuf, Option<Content>>) {
    // `symlink_metadata` does not follow a link: a link, even one in place of
    // the whole folder, is one entry, and the files it points to are not read.
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            files.insert(
                path.to_path_buf(),
                Some(Content::Folder(harness_platform::file::folder_mode(&meta))),
            );
            // An agent may have locked the folder (`chmod 500`) so that
            // nothing in it can be read or removed. When unlocking fails,
            // what is still locked shows up as unreadable, or later as not
            // put back, so the error itself is not needed.
            let _ = harness_platform::file::unlock_folder(path);
            match fs::read_dir(path) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry) => add_tree(&entry.path(), files),
                            Err(e) => unreadable(path, &e, files),
                        }
                    }
                }
                Err(e) => unreadable(path, &e, files),
            }
        }
        Ok(_) => {
            let content = read(path).unwrap_or_else(|e| Some(Content::Unreadable(e.kind())));
            files.insert(path.to_path_buf(), content);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => unreadable(path, &e, files),
    }
}

/// Notes that `path` exists but cannot be read.
fn unreadable(path: &Path, error: &io::Error, files: &mut BTreeMap<PathBuf, Option<Content>>) {
    files.insert(path.to_path_buf(), Some(Content::Unreadable(error.kind())));
}

/// What `path` holds now, or `None` if it does not exist.
pub(super) fn read(path: &Path) -> io::Result<Option<Content>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.is_symlink() {
        return Ok(Some(Content::Link(fs::read_link(path)?)));
    }
    if !meta.is_file() {
        return Ok(Some(Content::Special));
    }
    if meta.len() > MAX_KEPT_BYTES {
        return Ok(Some(Content::Summary {
            len: meta.len(),
            modified: meta.modified().ok(),
        }));
    }
    Ok(Some(Content::File {
        bytes: fs::read(path)?,
        permissions: meta.permissions(),
    }))
}

/// Writes `before` back to `path`, or removes `path` if it did not exist.
/// `Err` when it could not, or when no copy of `before` was kept.
pub(super) fn put_back(path: &Path, before: Option<&Content>) -> io::Result<()> {
    // A link or file the agent put there is removed first. `remove` also copes with what the
    // agent may have made hard to remove (a read-only file, a locked folder).
    match harness_platform::file::remove(path) {
        Ok(()) => {}
        // `NotADirectory`: a part of the path is a file, not a folder. The
        // system says so (ENOTDIR) for a file inside a folder the agent made
        // in place of a file: the paths are sorted, so that file was already
        // put back just before and the folder is gone. Nothing is here.
        // (Windows calls this "path not found", which is `NotFound`.)
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) => {}
        Err(e) => return Err(e),
    }
    let before = match before {
        None => return Ok(()),
        Some(content @ (Content::File { .. } | Content::Folder(_) | Content::Link(_))) => content,
        // No copy was kept, so the changed file can only be removed.
        Some(Content::Summary { .. } | Content::Special | Content::Unreadable(_)) => {
            return Err(io::Error::other(
                "no copy was kept (too big, unreadable or not a plain file); it is removed",
            ))
        }
    };
    // The agent may have removed the folder the file was in.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match before {
        Content::File { bytes, permissions } => {
            // `create_new` fails if something appeared here since `remove`
            // (a link from a process the agent left running): the bytes are
            // never written through it to another file.
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            file.write_all(bytes)?;
            // A new file gets the default permissions (not executable);
            // `clone` because `set_permissions` takes them by value and the
            // snapshot keeps its copy.
            file.set_permissions(permissions.clone())
        }
        // Its permissions come back at the end, with all the other folders'.
        Content::Folder(_) => fs::create_dir(path),
        Content::Link(target) => harness_platform::file::make_link(target, path),
        Content::Summary { .. } | Content::Special | Content::Unreadable(_) => Ok(()),
    }
}

/// Gives every folder in `files` that is still a folder the access rights
/// noted there, and returns those it could not. Folders at or below a path
/// in `skip` are left alone: something there could not be put back, maybe a
/// link to elsewhere, and a folder below it would be another one. The
/// deepest folders come first: a parent made read-only again would not let
/// its folders change.
pub(super) fn put_back_folder_modes(
    files: &BTreeMap<PathBuf, Option<Content>>,
    skip: &[PathBuf],
) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    // The map is sorted, so a folder comes before what is in it; `rev` turns that round.
    for (path, content) in files.iter().rev() {
        let Some(Content::Folder(mode)) = content else {
            continue;
        };
        if skip.iter().any(|skipped| path.starts_with(skipped)) {
            continue;
        }
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            // It was not made again; that failure is already listed.
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                failed.push(path.clone());
                continue;
            }
        };
        // Only a real folder is changed, never what a link points to.
        if !meta.is_dir() || harness_platform::file::folder_mode(&meta) == *mode {
            continue;
        }
        if harness_platform::file::set_folder_mode(path, *mode).is_err() {
            failed.push(path.clone());
        }
    }
    failed
}
