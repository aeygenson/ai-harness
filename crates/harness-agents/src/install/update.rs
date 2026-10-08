//! Updating the harness itself: is there a newer release on GitHub, and
//! putting it in the place of this program (`harness update`, «Update
//! Harness» in the TUI).
//!
//! Only a program built by the release workflow replaces itself: that build
//! sets `HARNESS_RELEASE_TARGET` (for example `x86_64-unknown-linux-musl`),
//! which also names the archive to download. A harness built from the source
//! (`cargo install`, `install.sh --dev`) is updated the way it was built.
//!
//! Each release carries `version.txt` (its number), `SHA256SUMS.txt` and one
//! archive per system. The archive is checked against `SHA256SUMS.txt`
//! before anything is replaced.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::catalog::{older, parse_version};
use super::models::run_for;

/// The version of this harness (every crate of the workspace has the same one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The harness's Releases page; `<it>/latest/download/<file>` is a file of the newest release.
pub const RELEASES_URL: &str = "https://github.com/aeygenson/ai-harness/releases";

/// What to say when this harness was not built by the release workflow.
pub const FROM_SOURCE: &str = "this harness was built from the source code, so it does not \
replace itself: get the new code (`git pull`) and build it again the way it was built \
(`cargo install --path crates/harness-cli --locked`, or on macOS and Linux the installer with --dev)";

/// Asking for the newest version number is quick; longer means the network is in trouble.
const CHECK_LIMIT: Duration = Duration::from_secs(20);
/// An archive is a few megabytes: a slow connection gets a few minutes.
const DOWNLOAD_LIMIT: Duration = Duration::from_mins(5);

/// Where the releases are, which of their archives fits here, and the
/// programs that download and unpack it. Tests point it at a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Releases {
    /// The Releases page, or a `file://` folder laid out the same way.
    pub url: String,
    /// The system this program was built for; `None` when built from the source.
    pub target: Option<String>,
    /// The version running now.
    pub current: String,
    /// The `curl` program that downloads.
    pub curl: PathBuf,
    /// The `tar` program that unpacks (`.tar.gz`, and `.zip` on Windows).
    pub tar: PathBuf,
}

impl Default for Releases {
    fn default() -> Self {
        Self {
            url: RELEASES_URL.to_string(),
            // Set only by `.github/workflows/release.yml` while it builds.
            target: option_env!("HARNESS_RELEASE_TARGET").map(str::to_string),
            current: VERSION.to_string(),
            curl: harness_platform::program::resolve("curl"),
            tar: harness_platform::program::tar(),
        }
    }
}

impl Releases {
    /// Can this program replace itself? Only a release build can.
    pub fn from_source(&self) -> bool {
        self.target.is_none()
    }
}

/// The number of the newest release, such as `0.5.0` (asks GitHub).
///
/// # Errors
/// Without a connection, or when the page gives no version number.
pub fn latest_version(releases: &Releases) -> Result<String, String> {
    let url = format!("{}/latest/download/version.txt", releases.url);
    let text = download_text(releases, &url)?;
    parse_version(text.trim()).ok_or_else(|| "the Releases page gave no version number".to_string())
}

/// The newest release's number when it is newer than this harness, `None` when this is the newest.
///
/// # Errors
/// As [`latest_version`].
pub fn newer_version(releases: &Releases) -> Result<Option<String>, String> {
    let latest = latest_version(releases)?;
    Ok(older(&releases.current, &latest).then_some(latest))
}

/// Downloads the newest release and puts it in the place of `program` (this
/// harness's own file). Tells each step through `on_line`. Returns the new
/// version, or `None` when this already is the newest.
///
/// The harness that runs now keeps running the old version until it is
/// started again.
///
/// # Errors
/// A harness built from the source ([`FROM_SOURCE`]), no connection, a
/// download that does not match `SHA256SUMS.txt`, or a folder that cannot
/// be written to. On any error `program` stays as it was.
pub fn update(
    releases: &Releases,
    program: &Path,
    mut on_line: impl FnMut(String),
) -> Result<Option<String>, String> {
    let target = releases
        .target
        .as_deref()
        .ok_or_else(|| FROM_SOURCE.to_string())?;
    on_line(format!("This harness: {}", releases.current));
    let Some(version) = newer_version(releases)? else {
        on_line("It is the newest version.".to_string());
        return Ok(None);
    };
    let name = archive_name(target);
    on_line(format!("Downloading {version}: {name}"));
    let tmp = tempfile::tempdir().map_err(|e| format!("cannot make a temporary folder: {e}"))?;
    let base = format!("{}/download/v{version}", releases.url);
    let archive = tmp.path().join(&name);
    download(releases, &format!("{base}/{name}"), &archive)?;
    let sums = download_text(releases, &format!("{base}/SHA256SUMS.txt"))?;
    check_sum(&archive, &name, &sums)?;
    on_line("The download matches SHA256SUMS.txt.".to_string());
    let unpacked = tmp.path().join("unpacked");
    fs::create_dir(&unpacked).map_err(|e| e.to_string())?;
    unpack(releases, &archive, &unpacked)?;
    let new = unpacked.join(program_name(target));
    if !new.is_file() {
        return Err(format!("{name} has no {}", program_name(target)));
    }
    replace(program, &new)?;
    on_line(format!(
        "Updated {} -> {version}: {}",
        releases.current,
        program.display()
    ));
    Ok(Some(version))
}

/// The release archive for `target`: a `.zip` for Windows, a `.tar.gz` for the others.
fn archive_name(target: &str) -> String {
    if target.contains("windows") {
        format!("harness-{target}.zip")
    } else {
        format!("harness-{target}.tar.gz")
    }
}

/// The program's file name inside the archive for `target`.
fn program_name(target: &str) -> &'static str {
    if target.contains("windows") {
        "harness.exe"
    } else {
        "harness"
    }
}

/// `curl` that stops at an HTTP error (`-f`), follows GitHub's redirects (`-L`)
/// and waits at most `limit`.
fn curl(releases: &Releases, limit: Duration) -> Command {
    let mut command = Command::new(&releases.curl);
    command
        .args(["-fsSL", "-m"])
        .arg(limit.as_secs().to_string());
    command
}

/// What `url` holds, as text.
fn download_text(releases: &Releases, url: &str) -> Result<String, String> {
    let mut command = curl(releases, CHECK_LIMIT);
    command.arg(url);
    run_for(command, "", &[], CHECK_LIMIT).map_err(|e| format!("cannot download {url}: {e}"))
}

/// Saves `url` as the file `to`.
fn download(releases: &Releases, url: &str, to: &Path) -> Result<(), String> {
    let mut command = curl(releases, DOWNLOAD_LIMIT);
    command.arg("-o").arg(to).arg(url);
    run_for(command, "", &[], DOWNLOAD_LIMIT)
        .map(|_| ())
        .map_err(|e| format!("cannot download {url}: {e}"))
}

/// Is the SHA-256 of `file` the one `sums` (lines «<hex>  <name>») gives for `name`?
fn check_sum(file: &Path, name: &str, sums: &str) -> Result<(), String> {
    let expected = sums
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        // `sha256sum` marks a file read in binary mode with `*` before its name.
        .find(|(_, file_name)| file_name.trim().trim_start_matches('*') == name)
        .map(|(hash, _)| hash.trim().to_ascii_lowercase())
        .ok_or_else(|| format!("SHA256SUMS.txt does not list {name}"))?;
    let data = fs::read(file).map_err(|e| e.to_string())?;
    if sha256_hex(&data) == expected {
        Ok(())
    } else {
        Err(format!(
            "{name} does not match SHA256SUMS.txt (a broken download?); nothing was changed"
        ))
    }
}

/// The SHA-256 of `data` in hexadecimal, the way `sha256sum` prints it.
fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            // Writing into a String cannot fail.
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Unpacks `archive` into the folder `into`.
fn unpack(releases: &Releases, archive: &Path, into: &Path) -> Result<(), String> {
    let mut command = Command::new(&releases.tar);
    command.arg("-xf").arg(archive).arg("-C").arg(into);
    run_for(command, "", &[], CHECK_LIMIT)
        .map(|_| ())
        .map_err(|e| format!("cannot unpack the download: {e}"))
}

/// Puts the file `new` in the place of `program`.
///
/// A running program cannot be overwritten on Windows, but it can be renamed;
/// so the old file is first moved aside to `<name>.old`, then the new one
/// takes its name. The old file is removed at once where the system allows it
/// (Linux, macOS), otherwise at the next update. If a step fails, the old
/// file goes back.
pub fn replace(program: &Path, new: &Path) -> Result<(), String> {
    let staged = with_suffix(program, ".new");
    let old = with_suffix(program, ".old");
    // Copied next to the program first: a rename works only within one disk.
    fs::copy(new, &staged).map_err(|e| format!("cannot write {}: {e}", staged.display()))?;
    // The same permissions as the old file: on Linux and macOS, runnable.
    if let Ok(metadata) = fs::metadata(program) {
        let _ = fs::set_permissions(&staged, metadata.permissions());
    }
    // Left by an earlier update on Windows; it no longer runs now.
    let _ = fs::remove_file(&old);
    if let Err(error) = fs::rename(program, &old) {
        let _ = fs::remove_file(&staged);
        return Err(format!("cannot move {} aside: {error}", program.display()));
    }
    if let Err(error) = fs::rename(&staged, program) {
        let _ = fs::rename(&old, program);
        let _ = fs::remove_file(&staged);
        return Err(format!("cannot put the new {}: {error}", program.display()));
    }
    let _ = fs::remove_file(&old);
    Ok(())
}

/// `program` with `suffix` added to its file name: `harness.exe` -> `harness.exe.old`.
fn with_suffix(program: &Path, suffix: &str) -> PathBuf {
    let mut name = program.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    program.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder laid out like the Releases page, with release `version`
    /// holding the archive for `target` with `program` in it.
    fn release_folder(root: &Path, version: &str, target: &str, program: &str) -> String {
        let latest = root.join("latest/download");
        fs::create_dir_all(&latest).unwrap();
        fs::write(latest.join("version.txt"), format!("{version}\n")).unwrap();
        let files = root.join("files");
        fs::create_dir_all(&files).unwrap();
        fs::write(files.join(program_name(target)), program).unwrap();
        let tagged = root.join(format!("download/v{version}"));
        fs::create_dir_all(&tagged).unwrap();
        let name = archive_name(target);
        // `-a` picks the format by the name, the same on GNU tar and bsdtar.
        let status = Command::new(harness_platform::program::tar())
            .arg("-a")
            .arg("-cf")
            .arg(tagged.join(&name))
            .arg("-C")
            .arg(&files)
            .arg(program_name(target))
            .status()
            .unwrap();
        assert!(status.success());
        let hash = sha256_hex(&fs::read(tagged.join(&name)).unwrap());
        fs::write(
            tagged.join("SHA256SUMS.txt"),
            format!("{hash}  {name}\nabc  other.zip\n"),
        )
        .unwrap();
        // `file:///C:/...` on Windows, `file:///tmp/...` elsewhere.
        let path = root.to_string_lossy().replace('\\', "/");
        format!("file:///{}", path.trim_start_matches('/'))
    }

    fn releases(url: String, current: &str) -> Releases {
        Releases {
            url,
            target: Some("x86_64-unknown-linux-musl".into()),
            current: current.into(),
            ..Releases::default()
        }
    }

    #[test]
    fn the_archive_and_program_names_follow_the_target() {
        assert_eq!(
            archive_name("x86_64-pc-windows-msvc"),
            "harness-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            archive_name("aarch64-apple-darwin"),
            "harness-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(program_name("x86_64-pc-windows-msvc"), "harness.exe");
        assert_eq!(program_name("aarch64-unknown-linux-musl"), "harness");
    }

    #[test]
    fn a_newer_release_replaces_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let url = release_folder(
            &dir.path().join("releases"),
            "9.9.9",
            "x86_64-unknown-linux-musl",
            "new",
        );
        let program = dir.path().join("bin/harness");
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, "old").unwrap();
        let mut lines = Vec::new();

        let updated = update(&releases(url, "0.4.0"), &program, |l| lines.push(l)).unwrap();

        assert_eq!(updated.as_deref(), Some("9.9.9"));
        assert_eq!(fs::read_to_string(&program).unwrap(), "new");
        assert!(
            lines.iter().any(|l| l.contains("matches SHA256SUMS")),
            "{lines:?}"
        );
        // Nothing is left next to it.
        let names: Vec<_> = fs::read_dir(program.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn the_newest_version_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let url = release_folder(
            &dir.path().join("releases"),
            "0.4.0",
            "x86_64-unknown-linux-musl",
            "new",
        );
        let program = dir.path().join("harness");
        fs::write(&program, "old").unwrap();

        let releases = releases(url, "0.4.0");
        assert_eq!(newer_version(&releases).unwrap(), None);
        assert_eq!(update(&releases, &program, |_| {}).unwrap(), None);
        assert_eq!(fs::read_to_string(&program).unwrap(), "old");
    }

    #[test]
    fn a_harness_built_from_source_does_not_replace_itself() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("harness");
        fs::write(&program, "old").unwrap();
        let releases = Releases {
            target: None,
            ..releases("file:///nowhere".into(), "0.4.0")
        };

        let error = update(&releases, &program, |_| {}).unwrap_err();

        assert!(releases.from_source());
        assert!(error.contains("built from the source"), "{error}");
        assert_eq!(fs::read_to_string(&program).unwrap(), "old");
    }

    #[test]
    fn a_download_that_does_not_match_its_sum_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("harness.tar.gz");
        fs::write(&file, "data").unwrap();

        let wrong = check_sum(&file, "harness.tar.gz", "00ff  harness.tar.gz\n").unwrap_err();
        let missing = check_sum(&file, "harness.tar.gz", "00ff  other.zip\n").unwrap_err();

        assert!(wrong.contains("does not match"), "{wrong}");
        assert!(missing.contains("does not list"), "{missing}");
        // The SHA-256 of "data", with `sha256sum`'s binary mark.
        let sum =
            "3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7 *harness.tar.gz";
        check_sum(&file, "harness.tar.gz", sum).unwrap();
    }

    #[test]
    fn replace_puts_the_old_file_back_when_the_new_one_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("harness");
        fs::write(&program, "old").unwrap();

        let error = replace(&program, &dir.path().join("missing")).unwrap_err();

        assert!(error.contains("cannot write"), "{error}");
        assert_eq!(fs::read_to_string(&program).unwrap(), "old");
        assert!(!with_suffix(&program, ".old").exists());
    }

    #[test]
    fn without_a_connection_the_check_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().replace('\\', "/");
        let url = format!("file:///{}", path.trim_start_matches('/'));

        let error = latest_version(&releases(url, "0.4.0")).unwrap_err();

        assert!(error.contains("cannot download"), "{error}");
    }
}
