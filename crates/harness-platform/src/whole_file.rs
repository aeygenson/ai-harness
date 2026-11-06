//! Writing a file whole, so that another harness window reading it at the
//! same moment sees either the old text or the new one, never half of it.
//!
//! The text goes into a new temporary file in the same folder first, which
//! is then renamed over the old file. A rename inside one folder replaces
//! the file in one step on Linux, macOS and Windows (Rust's `fs::rename`
//! replaces an existing file on all three).
//!
//! This alone does not stop two windows that read, change and write the
//! same file from losing one change: the one that writes last wins. Wrap
//! such work in [`crate::lock::wait_and_hold`].

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// How many temporary names are tried before giving up. Every name has the
/// process id in it, so a clash only happens with files a crash left behind.
const TEMP_NAME_ATTEMPTS: u32 = 100;

/// Writes `bytes` into `path` whole: readers see the old or the new file.
/// The folder must exist. On failure the old file stays as it was.
///
/// # Errors
///
/// The temporary file cannot be made or written, or the rename fails (on
/// Windows, for example, while another program holds the file open without
/// sharing it).
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} is not a file path", path.display())))?;
    // `parent` of a bare file name is an empty path, which means "here".
    let folder = match path.parent() {
        Some(folder) if !folder.as_os_str().is_empty() => folder,
        _ => Path::new("."),
    };
    let (temp, mut file) = new_temp_file(folder, name)?;
    // `sync_all`: the text is on the disk before the rename makes it the
    // file, so a power cut does not leave an empty file in its place.
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    // Closed before the rename: Windows cannot rename a file that is open.
    drop(file);
    let result = written.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Makes a new, empty file `.<name>.<pid>-<n>.tmp` in `folder`.
/// `create_new` fails when the name is taken, so two writers never share one.
fn new_temp_file(folder: &Path, name: &std::ffi::OsStr) -> io::Result<(PathBuf, File)> {
    let pid = std::process::id();
    for attempt in 0..TEMP_NAME_ATTEMPTS {
        let mut temp_name = OsString::from(".");
        temp_name.push(name);
        temp_name.push(format!(".{pid}-{attempt}.tmp"));
        let temp = folder.join(temp_name);
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(format!(
        "cannot make a temporary file for {} in {}",
        name.to_string_lossy(),
        folder.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.toml");

        write(&path, b"new").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn an_old_file_is_replaced_and_no_temporary_file_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.toml");
        fs::write(&path, "a much longer old text").unwrap();

        write(&path, b"short").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "short");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [OsString::from("projects.toml")]);
    }

    #[test]
    fn a_temporary_file_left_by_a_crash_is_stepped_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tui.toml");
        let left = dir
            .path()
            .join(format!(".tui.toml.{}-0.tmp", std::process::id()));
        fs::write(&left, "left behind").unwrap();

        write(&path, b"theme").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "theme");
        assert_eq!(fs::read_to_string(&left).unwrap(), "left behind");
    }

    #[test]
    fn a_missing_folder_is_an_error_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("tui.toml");

        assert!(write(&path, b"x").is_err());
        assert!(!dir.path().join("missing").exists());
    }

    #[test]
    fn readers_never_see_half_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let small = "a".repeat(10);
        let big = "b".repeat(200_000);
        write(&path, small.as_bytes()).unwrap();

        // One thread keeps writing both texts in turn; the reader must only
        // ever find one of them whole.
        let writer_path = path.clone();
        let (writer_small, writer_big) = (small.clone(), big.clone());
        let writer = std::thread::spawn(move || {
            for _ in 0..50 {
                write(&writer_path, writer_big.as_bytes()).unwrap();
                write(&writer_path, writer_small.as_bytes()).unwrap();
            }
        });
        while !writer.is_finished() {
            match fs::read_to_string(&path) {
                Ok(text) => assert!(text == small || text == big, "{} bytes", text.len()),
                // On Windows a read can meet the rename and be refused; that
                // is not half a file. A missing file would be a real fault.
                Err(error) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied),
            }
        }
        writer.join().unwrap();
    }
}
