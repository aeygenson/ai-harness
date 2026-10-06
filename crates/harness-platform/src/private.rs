//! Files and folders only Lisa can read: logins, secrets, MCP sign-ins.
//!
//! On Linux and macOS that is mode 600 for a file and 700 for a folder. On
//! Windows the harness keeps these under the user's own profile
//! (`C:\Users\<name>\.harness`, and temporary folders in
//! `C:\Users\<name>\AppData\Local\Temp`), which Windows lets only that user
//! (and administrators) read, so there is nothing more to set.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Writes `bytes` into `path`, readable only by Lisa. On Linux and macOS the
/// file is created with mode 600 before anything is written into it.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // The mode only applies to a new file; an old one might be readable by others.
    restrict_file(path)?;
    file.write_all(bytes)
}

/// Makes an existing file readable only by Lisa.
pub fn restrict_file(path: &Path) -> io::Result<()> {
    set_mode(path, 0o600)
}

/// Makes an existing folder usable only by Lisa.
pub fn restrict_dir(path: &Path) -> io::Result<()> {
    set_mode(path, 0o700)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Is `path` readable only by Lisa? Always true where there are no Unix modes.
pub fn is_private(path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The read, write and run bits of the group and of all other users:
        // a private file has none of them set.
        const GROUP_AND_OTHER_BITS: u32 = 0o077;
        Ok(fs::metadata(path)?.permissions().mode() & GROUP_AND_OTHER_BITS == 0)
    }
    #[cfg(not(unix))]
    {
        fs::metadata(path).map(|_| true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_file_is_private_even_if_it_was_not_before() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        fs::write(&path, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!is_private(&path).unwrap());
        }
        write(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert!(is_private(&path).unwrap());
    }

    #[test]
    fn a_folder_can_be_made_private() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("login");
        fs::create_dir(&inner).unwrap();
        restrict_dir(&inner).unwrap();
        assert!(is_private(&inner).unwrap());
    }
}
