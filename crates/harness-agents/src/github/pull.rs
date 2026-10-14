//! Pull requests through `gh`: the one a branch already has, and a new one.

use std::path::Path;
use std::time::Duration;

use harness_core::task::pull_request::PullRequest;

use super::quiet_command;

/// How long one `gh` call about pull requests may take.
const GH_LIMIT: Duration = Duration::from_mins(1);

/// How many open pull requests with the branch's name are looked at: forks
/// can have a branch of the same name.
const LIST_LIMIT: usize = 20;

/// What `gh pr list --json number,url,isCrossRepository` prints for one
/// pull request.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    number: u64,
    url: String,
    /// The branch is in another repository (a fork), not in `repo` itself.
    is_cross_repository: bool,
}

/// The open pull request from the branch `branch` of `repo` (`owner/name`)
/// itself, if there is one. `gh` finds branches by name only, so a fork's
/// branch with the same name (anyone can make one) is left out; a closed or
/// merged one is old work, not this task's.
pub(super) fn find(gh: &Path, repo: &str, branch: &str) -> Result<Option<PullRequest>, String> {
    let mut command = quiet_command(gh);
    // `--name=value`: a value that starts with `-` cannot be taken for an option.
    command.args([
        "pr",
        "list",
        &format!("--repo={repo}"),
        &format!("--head={branch}"),
        "--state=open",
        "--json=number,url,isCrossRepository",
        &format!("--limit={LIST_LIMIT}"),
    ]);
    let out = crate::install::models::run_for(command, "", &[], GH_LIMIT)?;
    let listed: Vec<Listed> =
        serde_json::from_str(&out).map_err(|e| format!("gh pr list printed no list: {e}"))?;
    listed
        .into_iter()
        .find(|found| !found.is_cross_repository)
        .map(|found| checked(repo, found.number, &found.url))
        .transpose()
}

/// What a new pull request is made of.
pub(super) struct NewPullRequest<'a> {
    /// The repository as `owner/name`.
    pub repo: &'a str,
    /// The branch the work goes into, for example `main`.
    pub base: &'a str,
    /// The task's branch.
    pub branch: &'a str,
    /// One line.
    pub title: &'a str,
    /// Markdown; given to `gh` on its standard input, not on the command line.
    pub body: &'a str,
}

/// Opens a pull request; `gh pr create` prints its address.
pub(super) fn create(gh: &Path, new: &NewPullRequest) -> Result<PullRequest, String> {
    let mut command = quiet_command(gh);
    command.args([
        "pr",
        "create",
        &format!("--repo={}", new.repo),
        &format!("--base={}", new.base),
        &format!("--head={}", new.branch),
        &format!("--title={}", new.title),
        "--body-file=-",
    ]);
    let out = crate::install::models::run_for(command, new.body, &[], GH_LIMIT)?;
    let url = out
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default();
    let number = url
        .rsplit('/')
        .next()
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| format!("gh pr create did not print the pull request's address: {url:?}"))?;
    checked(new.repo, number, url)
}

/// A pull request from `gh`'s answer, only when its address is the one
/// GitHub gives `number` in `repo`: the address is shown and opened later.
fn checked(repo: &str, number: u64, url: &str) -> Result<PullRequest, String> {
    let expected = format!("https://github.com/{repo}/pull/{number}");
    if url == expected {
        Ok(PullRequest {
            number,
            url: expected,
        })
    } else {
        Err(format!(
            "gh gave an unexpected address for pull request {number}: {:?}",
            harness_core::text::safe_line(url, 200)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_pull_request_is_found_by_its_branch() {
        let bin = tempfile::tempdir().unwrap();
        let args = bin.path().join("args.txt");
        let script = format!(
            "save-args {}\nprint [{{\"number\":9,\"url\":\"https://github.com/mallory/app/pull/9\",\"isCrossRepository\":true}},{{\"number\":12,\"url\":\"https://github.com/lisa/app/pull/12\",\"isCrossRepository\":false}}]\n",
            args.display()
        );
        let gh = harness_fake::install(bin.path(), "gh", &script);

        let found = find(&gh, "lisa/app", "harness/task-001").unwrap();

        assert_eq!(
            found,
            Some(PullRequest {
                number: 12,
                url: "https://github.com/lisa/app/pull/12".into()
            })
        );
        let saved = std::fs::read_to_string(&args).unwrap();
        assert!(saved.contains("--head=harness/task-001\n"), "{saved}");
        assert!(saved.contains("--state=open\n"), "{saved}");
    }

    #[test]
    fn no_pull_request_or_a_strange_address() {
        let bin = tempfile::tempdir().unwrap();
        let none = harness_fake::install(bin.path(), "gh", "print []\n");
        assert_eq!(find(&none, "lisa/app", "harness/task-001").unwrap(), None);

        let other = tempfile::tempdir().unwrap();
        let script =
            "print [{\"number\":12,\"url\":\"https://evil.example/pull/12\",\"isCrossRepository\":false}]\n";
        let strange = harness_fake::install(other.path(), "gh", script);
        let error = find(&strange, "lisa/app", "harness/task-001").unwrap_err();
        assert!(error.contains("unexpected address"), "{error}");
    }

    #[test]
    fn a_new_pull_request_gets_its_text_on_standard_input() {
        let bin = tempfile::tempdir().unwrap();
        let args = bin.path().join("args.txt");
        let body = bin.path().join("body.md");
        let script = format!(
            "save-args {}\nsave-stdin {}\nprint Creating pull request\nprint https://github.com/lisa/app/pull/7\n",
            args.display(),
            body.display()
        );
        let gh = harness_fake::install(bin.path(), "gh", &script);
        let new = NewPullRequest {
            repo: "lisa/app",
            base: "main",
            branch: "harness/task-001",
            title: "-Add a parser",
            body: "## Task\n",
        };

        let made = create(&gh, &new).unwrap();

        assert_eq!(made.number, 7);
        let saved = std::fs::read_to_string(&args).unwrap();
        assert_eq!(
            saved.lines().collect::<Vec<_>>(),
            [
                "pr",
                "create",
                "--repo=lisa/app",
                "--base=main",
                "--head=harness/task-001",
                "--title=-Add a parser",
                "--body-file=-"
            ]
        );
        assert_eq!(std::fs::read_to_string(&body).unwrap(), "## Task\n");
    }
}
