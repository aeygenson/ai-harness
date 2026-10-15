//! Updating a harness built from the source code (`cargo install`,
//! `install.sh --dev`): get the new commits of its git folder
//! (`git pull --ff-only`), build it again the way the installer does
//! (`cargo install`) and put the new program in the place of this one.
//!
//! The folder is the one this program was compiled from. Nothing is pulled
//! when the folder has changes of its own that do not let git move forward:
//! `git pull --ff-only` refuses and says why.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::{replace, Releases};
use crate::install::catalog::parse_version;
use crate::install::models::run_for;

/// Asking the git server for new commits is quick; longer means the network is in trouble.
const FETCH_LIMIT: Duration = Duration::from_mins(1);
/// A full build of the harness takes a few minutes on a slow laptop; this is plenty.
const BUILD_LIMIT: Duration = Duration::from_mins(30);

/// The file in the folder whose `version` is the harness's version.
const CLI_MANIFEST: &str = "crates/harness-cli/Cargo.toml";

/// The git folder this program was compiled from, if it is still there.
pub(super) fn checkout_of_this_build() -> Option<PathBuf> {
    // `CARGO_MANIFEST_DIR` is this crate's folder at compile time:
    // `<checkout>/crates/harness-agents`, so the checkout is two levels up.
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    checkout
        .join(".git")
        .exists()
        .then(|| checkout.to_path_buf())
}

/// What the new commits of `checkout` bring, such as `0.7.1 (a1b2c3d)`:
/// their version and the newest commit. `None` when there are none.
///
/// # Errors
/// No connection to the git server, or a folder git cannot read.
pub(super) fn newer_code(releases: &Releases, checkout: &Path) -> Result<Option<String>, String> {
    git(releases, checkout, &["fetch", "--quiet"])?;
    let count = git(
        releases,
        checkout,
        &["rev-list", "--count", "HEAD..@{upstream}"],
    )?;
    if count.trim() == "0" {
        return Ok(None);
    }
    let commit = git(releases, checkout, &["rev-parse", "--short", "@{upstream}"])?;
    let manifest = git(
        releases,
        checkout,
        &["show", &format!("@{{upstream}}:{CLI_MANIFEST}")],
    )?;
    let version = package_version(&manifest).unwrap_or_else(|| releases.current.clone());
    Ok(Some(format!("{version} ({})", commit.trim())))
}

/// Pulls the new commits of `checkout`, builds them and puts the new program
/// in the place of `program`. Tells each step through `on_line`. Returns
/// what was installed, or `None` when there were no new commits.
///
/// # Errors
/// As [`newer_code`]; also a pull git refuses (changes of your own), a
/// failed build, or a folder that cannot be written to. On any error
/// `program` stays as it was.
pub(super) fn update(
    releases: &Releases,
    checkout: &Path,
    program: &Path,
    on_line: &mut dyn FnMut(String),
) -> Result<Option<String>, String> {
    on_line(format!(
        "This harness: {}, built from {}",
        releases.current,
        checkout.display()
    ));
    let Some(label) = newer_code(releases, checkout)? else {
        on_line("Its folder has no new commits.".to_string());
        return Ok(None);
    };
    on_line(format!(
        "Getting the new code ({label}): git pull --ff-only"
    ));
    git(releases, checkout, &["pull", "--ff-only"])?;
    on_line("Building it with cargo install; this takes a few minutes.".to_string());
    // Built into a temporary folder first: `program` changes only once the build worked.
    let built = tempfile::tempdir().map_err(|e| format!("cannot make a temporary folder: {e}"))?;
    let mut command = Command::new(&releases.cargo);
    command
        .current_dir(checkout)
        .args([
            "install",
            "--locked",
            "--path",
            "crates/harness-cli",
            "--root",
        ])
        .arg(built.path());
    run_for(command, "", &[], BUILD_LIMIT).map_err(|e| format!("the build failed: {e}"))?;
    let name = format!("harness{}", std::env::consts::EXE_SUFFIX);
    let new = built.path().join("bin").join(&name);
    if !new.is_file() {
        return Err(format!("the build made no {name}"));
    }
    replace(program, &new)?;
    on_line(format!(
        "Updated {} -> {label}: {}",
        releases.current,
        program.display()
    ));
    Ok(Some(label))
}

/// Runs `git -C checkout <args>` and returns what it printed.
fn git(releases: &Releases, checkout: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(&releases.git);
    command.arg("-C").arg(checkout).args(args);
    run_for(command, "", &[], FETCH_LIMIT).map_err(|e| format!("git {}: {e}", args.join(" ")))
}

/// The `version = "…"` of a `Cargo.toml`'s `[package]`.
fn package_version(manifest: &str) -> Option<String> {
    manifest
        .lines()
        .skip_while(|line| line.trim() != "[package]")
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .find_map(|line| {
            let value = line.trim().strip_prefix("version")?.trim_start();
            parse_version(value.strip_prefix('=')?.trim().trim_matches('"'))
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Runs git in `dir` and fails the test if git fails.
    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// Commits `crates/harness-cli/Cargo.toml` with `version` in `dir`.
    fn commit_version(dir: &Path, version: &str) {
        let manifest = dir.join(CLI_MANIFEST);
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let text = format!("[package]\nname = \"harness-cli\"\nversion = \"{version}\"\n");
        fs::write(manifest, text).unwrap();
        run_git(dir, &["add", "."]);
        run_git(dir, &["commit", "-q", "-m", version]);
    }

    /// An "upstream" repository and a clone of it, the way a `--dev` install keeps its code.
    fn upstream_and_checkout(root: &Path) -> (PathBuf, PathBuf) {
        let upstream = root.join("upstream");
        fs::create_dir_all(&upstream).unwrap();
        run_git(&upstream, &["init", "-q", "-b", "main"]);
        commit_version(&upstream, "0.7.0");
        let checkout = root.join("checkout");
        let status = Command::new("git")
            .arg("clone")
            .arg("-q")
            .arg(&upstream)
            .arg(&checkout)
            .status()
            .unwrap();
        assert!(status.success());
        (upstream, checkout)
    }

    fn releases(cargo: PathBuf) -> Releases {
        Releases {
            target: None,
            current: "0.7.0".into(),
            git: PathBuf::from("git"),
            cargo,
            ..Releases::default()
        }
    }

    #[test]
    fn new_commits_are_built_and_replace_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let (upstream, checkout) = upstream_and_checkout(dir.path());
        commit_version(&upstream, "0.7.1");
        let name = format!("harness{}", std::env::consts::EXE_SUFFIX);
        let script = format!(
            "need-args install --locked --path crates/harness-cli --root ${{after:--root}} else 3\n\
             need-text crates/harness-cli/Cargo.toml 0.7.1 else 4\n\
             save-text ${{after:--root}}/bin/{name} new\n"
        );
        let cargo = harness_fake::install(&dir.path().join("bin"), "cargo", &script);
        let program = dir.path().join("installed").join(&name);
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, "old").unwrap();
        let mut lines = Vec::new();

        let updated = update(&releases(cargo), &checkout, &program, &mut |l| {
            lines.push(l);
        });

        let label = updated.unwrap().unwrap();
        assert!(label.starts_with("0.7.1 ("), "{label}");
        assert_eq!(fs::read_to_string(&program).unwrap(), "new");
        assert!(lines.iter().any(|l| l.contains("git pull")), "{lines:?}");
    }

    #[test]
    fn without_new_commits_nothing_is_built() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, checkout) = upstream_and_checkout(dir.path());
        // A build would fail the test: the fake cargo exits with an error.
        let cargo = harness_fake::install(&dir.path().join("bin"), "cargo", "exit 9\n");
        let program = dir.path().join("harness");
        fs::write(&program, "old").unwrap();
        let releases = releases(cargo);

        assert_eq!(newer_code(&releases, &checkout).unwrap(), None);
        assert_eq!(
            update(&releases, &checkout, &program, &mut |_| {}).unwrap(),
            None
        );
        assert_eq!(fs::read_to_string(&program).unwrap(), "old");
    }

    #[test]
    fn a_failed_build_leaves_the_program_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let (upstream, checkout) = upstream_and_checkout(dir.path());
        commit_version(&upstream, "0.7.1");
        let cargo = harness_fake::install(
            &dir.path().join("bin"),
            "cargo",
            "eprint error: could not compile\nexit 101\n",
        );
        let program = dir.path().join("harness");
        fs::write(&program, "old").unwrap();

        let error = update(&releases(cargo), &checkout, &program, &mut |_| {}).unwrap_err();

        assert!(error.contains("the build failed"), "{error}");
        assert_eq!(fs::read_to_string(&program).unwrap(), "old");
    }

    #[test]
    fn the_version_is_read_from_the_package_section_only() {
        let manifest = "[workspace]\nversion = \"9.9.9\"\n\n[package]\nname = \"x\"\n\
                        version = \"0.7.1\"\n\n[dependencies]\nversion = \"1\"\n";

        assert_eq!(package_version(manifest).as_deref(), Some("0.7.1"));
        assert_eq!(package_version("[dependencies]\nserde = \"1\"\n"), None);
    }
}
