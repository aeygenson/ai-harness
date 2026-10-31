//! The repository's own settings file (`.git/config`): reading it before a
//! role and putting it back after one, without writing through a link that
//! an agent put there.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use super::{GitError, Repo};

/// A settings file bigger than this is not read: git's own file is a few
/// KB, and an agent could make a huge (or sparse) one so that reading it
/// runs the harness out of memory before the old settings are back.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Where the new settings are written before they replace the old file in
/// one step (see [`put_back`](LocalConfig::put_back)).
const NEW_FILE_NAME: &str = "config.harness-new";

/// What `.git/config` was at one moment.
///
/// An agent with a shell could replace the file with a link to another file
/// (a shell start-up file, say). Writing through such a link would
/// overwrite that other file, so a link the agent made is removed, never
/// written through.
#[derive(Clone, PartialEq, Eq)]
pub enum LocalConfig {
    /// There was no settings file.
    Missing,
    /// A plain file: what it held, and its permissions. The permissions count
    /// too: a file that others may write to lets them add a command to it,
    /// and one the user keeps private may hold a password in a remote's
    /// address. (On Windows only the read-only flag is compared; ACLs are not.)
    File {
        /// The whole content.
        bytes: Vec<u8>,
        /// Who may read and write it.
        permissions: fs::Permissions,
    },
    /// A symbolic link the user made, where it points, and what the file
    /// there held (`None` when it could not be read): `git config` changes
    /// that file, not the link.
    Link {
        /// Where the link points.
        target: PathBuf,
        /// What the file it points to held.
        bytes: Option<Vec<u8>>,
    },
    /// Something else: a folder, a named pipe (FIFO), a socket or a device.
    /// It is never opened: reading a pipe could wait forever.
    Other,
}

/// Prints only the sizes: the settings may hold a password in a remote's address.
impl fmt::Debug for LocalConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "Missing"),
            Self::File { bytes, permissions } => {
                write!(f, "File({} bytes, {permissions:?})", bytes.len())
            }
            Self::Link { target, bytes } => write!(
                f,
                "Link({}, {:?} bytes)",
                target.display(),
                bytes.as_ref().map(Vec::len)
            ),
            Self::Other => write!(f, "Other"),
        }
    }
}

impl LocalConfig {
    /// What the settings file at `path` is now (see [`Repo::local_config_path`]).
    ///
    /// The harness reads it before and after each role: an agent with a shell
    /// could add a setting there that makes git run a command of its choice
    /// (`core.fsmonitor`, a `filter` driver) the next time anyone runs git.
    /// An error also when the file is too big or its folder is a link.
    pub fn read(path: &Path) -> Result<Self, GitError> {
        read_now(path).map_err(|source| GitError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Makes the settings file at `path` what `self` was again. No git runs:
    /// git refuses to start at all while the settings are broken.
    ///
    /// A file is written next to it first and then renamed over it, so the
    /// old settings are never lost half way, and a rename replaces a link
    /// instead of writing through it.
    pub fn put_back(&self, path: &Path) -> Result<(), GitError> {
        make_again(path, self).map_err(|source| GitError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

impl Repo {
    /// Where the repository's own settings file (`.git/config`) is. Git says
    /// so itself (`.git` can be a file pointing elsewhere), so this is asked
    /// while the settings still work: before a role.
    pub fn local_config_path(&self) -> Result<PathBuf, GitError> {
        self.git_path("config")
    }

    /// The repository's own settings file as it is now (see [`LocalConfig::read`]).
    pub fn local_config(&self) -> Result<LocalConfig, GitError> {
        LocalConfig::read(&self.local_config_path()?)
    }
}

/// An error unless the folder `path` is in is a real folder. An agent could
/// replace `.git` with a link to another folder (`~/.ssh`, say): its
/// `config` would then be read and overwritten. (Folders further up, such
/// as the project itself, are not checked.)
fn check_folder(path: &Path) -> io::Result<()> {
    let folder = path.parent().unwrap_or(Path::new("."));
    let meta = fs::symlink_metadata(folder)?;
    if meta.is_dir() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} is not a plain folder (maybe a link); fix it by hand",
            folder.display()
        )))
    }
}

/// Reads the plain file at `path` (a link is followed), at most
/// [`MAX_CONFIG_BYTES`]: `take` stops also a file that grows while it is read.
fn read_limited(path: &Path, meta: &fs::Metadata) -> io::Result<Vec<u8>> {
    let too_big = || io::Error::other("the settings file is too big to be git's own");
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(too_big());
    }
    Ok(bytes)
}

/// What `path` is now. `symlink_metadata` does not follow a link.
fn read_now(path: &Path) -> io::Result<LocalConfig> {
    check_folder(path)?;
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LocalConfig::Missing),
        Err(e) => return Err(e),
    };
    if meta.is_symlink() {
        // Only a plain file is read through the link: a pipe could wait forever.
        let bytes = match fs::metadata(path) {
            Ok(target_meta) if target_meta.is_file() => read_limited(path, &target_meta).ok(),
            _ => None,
        };
        return Ok(LocalConfig::Link {
            target: fs::read_link(path)?,
            bytes,
        });
    }
    if !meta.is_file() {
        return Ok(LocalConfig::Other);
    }
    Ok(LocalConfig::File {
        bytes: read_limited(path, &meta)?,
        permissions: meta.permissions(),
    })
}

/// Makes `path` what `before` was again.
fn make_again(path: &Path, before: &LocalConfig) -> io::Result<()> {
    check_folder(path)?;
    match before {
        LocalConfig::Missing => remove_if_there(path),
        LocalConfig::File { bytes, permissions } => replace_with_file(path, bytes, permissions),
        LocalConfig::Link { target, bytes } => {
            let now = read_now(path).ok();
            let same_link =
                matches!(&now, Some(LocalConfig::Link { target: t, .. }) if t == target);
            if !same_link {
                remove_if_there(path)?;
                return harness_platform::file::make_link(target, path);
            }
            // Writing the old settings through the link could overwrite any
            // file the agent made it point to, so the user fixes it by hand.
            if matches!(&now, Some(LocalConfig::Link { bytes: b, .. }) if b != bytes) {
                return Err(io::Error::other(format!(
                    "the file .git/config links to ({}) changed; check it by hand",
                    target.display()
                )));
            }
            Ok(())
        }
        // Nothing of it was kept, so it can only be removed.
        LocalConfig::Other => {
            remove_if_there(path)?;
            Err(io::Error::other(
                ".git/config was not a plain file before the role; it is removed",
            ))
        }
    }
}

/// Writes a new file next to `path` and renames it over `path`.
fn replace_with_file(path: &Path, bytes: &[u8], permissions: &fs::Permissions) -> io::Result<()> {
    let new = path.with_file_name(NEW_FILE_NAME);
    // A leftover from a run that stopped half way, or something an agent put there.
    remove_if_there(&new)?;
    let written = write_new(&new, bytes, permissions).and_then(|()| {
        // A file or a link is replaced in one step. A rename cannot replace
        // a folder, and on Windows not a read-only file: only then is the
        // old one removed first.
        fs::rename(&new, path).or_else(|_| {
            remove_if_there(path)?;
            fs::rename(&new, path)
        })
    });
    if written.is_err() {
        // Best effort: the error that matters is the one returned.
        let _ = fs::remove_file(&new);
    }
    written
}

/// Makes the file `path`, which must not exist yet, with these permissions and content.
fn write_new(path: &Path, bytes: &[u8], permissions: &fs::Permissions) -> io::Result<()> {
    // `create_new` fails if something appeared here since it was removed (a
    // link from a process the agent left running): nothing is written
    // through it to another file.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    // The permissions come before the content, so a private file is never
    // readable by others, not even for a moment. A read-only file can still
    // be written: the handle was opened for writing before. `clone` because
    // `set_permissions` takes them by value.
    file.set_permissions(permissions.clone())?;
    file.write_all(bytes)
}

/// Removes the file, link or folder at `path`; nothing there is fine.
fn remove_if_there(path: &Path) -> io::Result<()> {
    match harness_platform::file::remove(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, Repo, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let path = repo.local_config_path().unwrap();
        (dir, repo, path)
    }

    #[test]
    fn changed_settings_are_put_back() {
        let (_dir, repo, path) = repo();
        let before = LocalConfig::read(&path).unwrap();
        repo.git(&["config", "filter.agent.clean", "sh agent-script.sh"])
            .unwrap();
        assert_ne!(LocalConfig::read(&path).unwrap(), before);

        before.put_back(&path).unwrap();

        assert_eq!(LocalConfig::read(&path).unwrap(), before);
    }

    // Making a link needs extra rights on Windows, so this runs on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_settings_is_a_change_and_its_target_is_not_written() {
        let (dir, _repo, path) = repo();
        let before = LocalConfig::read(&path).unwrap();
        // The same settings, so only the link itself is the change.
        let elsewhere = dir.path().join("elsewhere");
        fs::copy(&path, &elsewhere).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        fs::write(&elsewhere, "the user's other file\n").unwrap();

        assert!(matches!(
            LocalConfig::read(&path).unwrap(),
            LocalConfig::Link { target, .. } if target == elsewhere
        ));
        before.put_back(&path).unwrap();

        assert_eq!(LocalConfig::read(&path).unwrap(), before);
        assert_eq!(
            fs::read_to_string(&elsewhere).unwrap(),
            "the user's other file\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn settings_opened_to_everybody_get_their_permissions_back() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, _repo, path) = repo();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let before = LocalConfig::read(&path).unwrap();
        // Any user of the computer could now add a command to it.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert_ne!(LocalConfig::read(&path).unwrap(), before);

        before.put_back(&path).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_folder_in_place_of_the_settings_is_removed() {
        let (_dir, _repo, path) = repo();
        let before = LocalConfig::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inside"), "planted").unwrap();

        assert_eq!(LocalConfig::read(&path).unwrap(), LocalConfig::Other);
        before.put_back(&path).unwrap();

        assert_eq!(LocalConfig::read(&path).unwrap(), before);
    }

    #[test]
    fn broken_settings_are_put_back_without_git() {
        let (_dir, repo, path) = repo();
        let before = LocalConfig::read(&path).unwrap();
        // Git refuses to start with this file, also to say where it is.
        fs::write(&path, "[core\n").unwrap();
        repo.local_config_path().unwrap_err();

        before.put_back(&path).unwrap();

        assert_eq!(repo.local_config().unwrap(), before);
    }

    #[test]
    fn a_huge_settings_file_is_not_read_but_can_be_replaced() {
        let (_dir, _repo, path) = repo();
        let before = LocalConfig::read(&path).unwrap();
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_CONFIG_BYTES + 1).unwrap();

        LocalConfig::read(&path).unwrap_err();
        before.put_back(&path).unwrap();

        assert_eq!(LocalConfig::read(&path).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_git_folder_is_neither_read_nor_written() {
        let (dir, _repo, path) = repo();
        let before = LocalConfig::read(&path).unwrap();
        // Another folder of the user with a `config` in it.
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("config"), "Host *\n").unwrap();
        fs::rename(dir.path().join(".git"), dir.path().join("moved")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(".git")).unwrap();

        LocalConfig::read(&path).unwrap_err();
        before.put_back(&path).unwrap_err();

        assert_eq!(
            fs::read_to_string(elsewhere.join("config")).unwrap(),
            "Host *\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn settings_behind_the_users_own_link_are_compared_but_not_written_through() {
        let (dir, _repo, path) = repo();
        let shared = dir.path().join("shared-config");
        fs::rename(&path, &shared).unwrap();
        std::os::unix::fs::symlink(&shared, &path).unwrap();
        let before = LocalConfig::read(&path).unwrap();
        // The agent points the user's file at another one of the user's files.
        let bashrc = dir.path().join("bashrc");
        fs::write(&bashrc, "alias ll='ls -l'\n").unwrap();
        fs::remove_file(&shared).unwrap();
        std::os::unix::fs::symlink(&bashrc, &shared).unwrap();
        assert_ne!(LocalConfig::read(&path).unwrap(), before);

        before.put_back(&path).unwrap_err();

        assert_eq!(fs::read_to_string(&bashrc).unwrap(), "alias ll='ls -l'\n");
        assert_eq!(fs::read_link(&path).unwrap(), shared);
    }

    #[cfg(unix)]
    #[test]
    fn the_users_own_link_is_made_again() {
        let (dir, _repo, path) = repo();
        let shared = dir.path().join("shared-config");
        fs::rename(&path, &shared).unwrap();
        std::os::unix::fs::symlink(&shared, &path).unwrap();
        let before = LocalConfig::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, "[core]\n\tfsmonitor = sh evil\n").unwrap();

        before.put_back(&path).unwrap();

        assert_eq!(LocalConfig::read(&path).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_settings_file_is_replaced() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, _repo, path) = repo();
        let before = LocalConfig::read(&path).unwrap();
        fs::write(&path, "[core]\n\tfsmonitor = sh evil\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();

        before.put_back(&path).unwrap();

        assert_eq!(LocalConfig::read(&path).unwrap(), before);
    }

    #[test]
    fn debug_does_not_print_the_settings() {
        let config = LocalConfig::File {
            bytes: b"url = https://lisa:secret@example.com".to_vec(),
            permissions: fs::metadata(".").unwrap().permissions(),
        };

        let shown = format!("{config:?}");

        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("37 bytes"), "{shown}");
    }
}
