//! Making a symbolic link, unlocking a folder and removing whatever is at a
//! path, which each system does in its own way. Used by the tripwire to put
//! git's hooks back after an agent, who may have made them hard to change on
//! purpose.

use std::fs;
use std::io;
use std::path::Path;

/// Makes `link` a symbolic link that points to `target`, as `ln -s` does.
/// `target` is kept as given: a relative one is read from `link`'s folder.
/// On Windows this may need Developer Mode or administrator rights.
pub fn make_link(target: &Path, link: &Path) -> io::Result<()> {
    make_link_here(target, link)
}

#[cfg(unix)]
fn make_link_here(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_link_here(target: &Path, link: &Path) -> io::Result<()> {
    // Windows has one kind of link for files and one for folders, and must
    // know which to make; a missing target counts as a file.
    let seen_from_link = link.parent().unwrap_or(Path::new("")).join(target);
    if seen_from_link.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// Removes the file, link or folder (with everything in it) at `path`; a
/// link is removed, never what it points to. Works also when it was made
/// hard to remove: a folder without write rights on Linux and macOS, a
/// read-only file or a folder link (junction) on Windows.
pub fn remove(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        unlock_tree(path)?;
        fs::remove_dir_all(path)
    } else {
        remove_one(path, &meta)
    }
}

/// Gives the user back the rights to list, enter and change `folder` and
/// every folder in it (at any depth), so that what is inside can be read,
/// removed and written again. Does nothing when `folder` is missing or is not
/// a folder: a link is never followed.
pub fn unlock(folder: &Path) -> io::Result<()> {
    match fs::symlink_metadata(folder) {
        Ok(meta) if meta.is_dir() => unlock_tree(folder),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Adds read, write and enter rights for the owner to `folder` (known to be
/// a folder, not a link) and to every folder inside it.
#[cfg(unix)]
fn unlock_tree(folder: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Read, write and enter, for the owner only (`rwx------` added to the mode).
    const OWNER_ALL: u32 = 0o700;
    let mut permissions = fs::symlink_metadata(folder)?.permissions();
    permissions.set_mode(permissions.mode() | OWNER_ALL);
    fs::set_permissions(folder, permissions)?;
    // One folder that cannot be unlocked (owned by root, say) must not leave
    // the others locked: go on, and return the first error at the end.
    let mut first_error = None;
    for entry in fs::read_dir(folder)? {
        // `file_type` does not follow links: a link to a folder is not entered.
        let unlocked = entry.and_then(|entry| {
            if entry.file_type()?.is_dir() {
                unlock_tree(&entry.path())
            } else {
                Ok(())
            }
        });
        if let Err(e) = unlocked {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Clears the read-only flag of `folder` (known to be a folder, not a link)
/// and of everything in it, at any depth: Windows refuses to remove or change
/// a read-only file, folder or junction.
#[cfg(windows)]
fn unlock_tree(folder: &Path) -> io::Result<()> {
    clear_read_only(folder, &fs::symlink_metadata(folder)?)?;
    // As on Linux: go on after an error, return the first one at the end.
    let mut first_error = None;
    for entry in fs::read_dir(folder)? {
        let unlocked = entry.and_then(|entry| {
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                unlock_tree(&entry.path())
            } else {
                clear_read_only(&entry.path(), &meta)
            }
        });
        if let Err(e) = unlocked {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Removes a file or a link. On Linux and macOS only the rights of the
/// folder it is in matter (see [`unlock`]).
#[cfg(unix)]
fn remove_one(path: &Path, _meta: &fs::Metadata) -> io::Result<()> {
    fs::remove_file(path)
}

/// Removes a file or a link. A link to a folder, and a junction (which any
/// user can make with `mklink /J`), are folders to Windows: `remove_dir`
/// removes the link itself, not the folder it points to.
#[cfg(windows)]
fn remove_one(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;

    clear_read_only(path, meta)?;
    if meta.file_type().is_symlink_dir() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

/// Clears the read-only flag of `path`, when it has one.
#[cfg(windows)]
#[expect(
    clippy::permissions_set_readonly_false,
    reason = "on Windows this only clears the read-only flag, so the file can be removed"
)]
fn clear_read_only(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    let mut permissions = meta.permissions();
    if !permissions.readonly() {
        return Ok(());
    }
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Making a link needs extra rights on Windows, so this runs on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn a_relative_link_keeps_its_target_as_given() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("script"), "echo hi").unwrap();
        let link = dir.path().join("hook");

        make_link(Path::new("script"), &link).unwrap();

        assert_eq!(fs::read_link(&link).unwrap(), Path::new("script"));
        assert_eq!(fs::read_to_string(&link).unwrap(), "echo hi");
    }

    #[test]
    fn a_read_only_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hook");
        fs::write(&file, "echo planted").unwrap();
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();

        remove(&file).unwrap();

        assert!(!file.exists());
    }

    #[test]
    fn a_folder_with_a_read_only_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("hook");
        fs::create_dir_all(folder.join("inner")).unwrap();
        let file = folder.join("inner/run");
        fs::write(&file, "echo planted").unwrap();
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();

        remove(&folder).unwrap();

        assert!(!folder.exists());
    }

    // As root (in a container) the rights are not checked at all, so there
    // this passes also without `unlock_tree`; as a user (CI) it does not.
    #[cfg(unix)]
    #[test]
    fn a_folder_without_write_rights_is_removed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("hook");
        fs::create_dir_all(folder.join("inner")).unwrap();
        fs::write(folder.join("inner/run"), "echo planted").unwrap();
        // Read and enter, but no write: what `chmod 500` leaves.
        for locked in [folder.join("inner"), folder.clone()] {
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        }

        remove(&folder).unwrap();

        assert!(!folder.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_folder_is_removed_and_the_folder_kept() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("keep"), "mine").unwrap();
        let link = dir.path().join("hook");
        make_link(&elsewhere, &link).unwrap();

        remove(&link).unwrap();

        fs::symlink_metadata(&link).unwrap_err();
        assert!(elsewhere.join("keep").exists());
    }

    // `mklink /J` makes a junction without administrator rights.
    #[cfg(windows)]
    #[test]
    fn a_junction_is_removed_and_its_folder_kept() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("keep"), "mine").unwrap();
        let junction = dir.path().join("hook");
        let made = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&elsewhere)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");

        remove(&junction).unwrap();

        fs::symlink_metadata(&junction).unwrap_err();
        assert!(elsewhere.join("keep").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unlock_gives_back_the_rights_on_every_folder_but_not_through_a_link() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("hooks");
        fs::create_dir_all(folder.join("inner")).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o500)).unwrap();
        make_link(&elsewhere, &folder.join("link")).unwrap();
        // List but not enter (`chmod 600`), and enter but not change (`chmod 500`).
        fs::set_permissions(folder.join("inner"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o500)).unwrap();

        unlock(&folder).unwrap();

        let mode = |path: &Path| fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&folder), 0o700);
        assert_eq!(mode(&folder.join("inner")), 0o700);
        assert_eq!(mode(&elsewhere), 0o500, "a link is not followed");
        fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn unlock_does_nothing_for_a_missing_folder() {
        let dir = tempfile::tempdir().unwrap();

        unlock(&dir.path().join("missing")).unwrap();
    }
}
