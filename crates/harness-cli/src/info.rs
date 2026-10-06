//! `harness models` and `harness agents`: what the installed agents offer.

use anyhow::{Context, Result};
use harness_agents::install::credentials;
use harness_core::config::{projects, AgentKind};

/// `harness models`: the saved lists, after asking the agents again with
/// `--refresh`.
pub(crate) fn models(refresh: bool) -> Result<()> {
    use harness_core::models;
    let home = projects::harness_home().context("no home folder found")?;
    if refresh {
        let dir = credentials::default_dir().context("no home folder found")?;
        println!("Asking the agents with a saved login…");
        for (agent, result) in harness_agents::install::models::ask_all(&dir, &Default::default()) {
            match result {
                Ok(list) => {
                    models::save(&home, &list)
                        .with_context(|| format!("cannot save the models of {agent}"))?;
                }
                Err(error) => println!("{agent}: {error}"),
            }
        }
    }
    let mut any = false;
    for agent in AgentKind::ALL {
        let Some(list) = models::load(&home, agent) else {
            continue;
        };
        any = true;
        println!("\n{agent}:");
        for model in &list.models {
            let default = if model.default { " (default)" } else { "" };
            let efforts = if model.efforts.is_empty() {
                String::new()
            } else {
                let levels: Vec<String> = model
                    .efforts
                    .iter()
                    .map(|e| {
                        if model.default_effort.as_ref() == Some(e) {
                            format!("[{e}]")
                        } else {
                            e.clone()
                        }
                    })
                    .collect();
                format!("  effort: {}", levels.join(" "))
            };
            let name = model
                .name
                .as_deref()
                .map(|n| format!("  {n}"))
                .unwrap_or_default();
            println!("  {}{default}{name}{efforts}", model.id);
        }
    }
    if !any {
        println!("No model lists yet: run `harness models --refresh`.");
    }
    Ok(())
}

/// The catalog of agents with what was found on this computer.
pub(crate) fn agents() {
    use harness_agents::install::catalog;
    let dir = credentials::default_dir();
    for status in catalog::check_all(dir.as_deref()) {
        let entry = &status.entry;
        let mark = if status.old() {
            "!"
        } else if status.installed() {
            "✓"
        } else {
            "○"
        };
        let runs = if entry.runs() {
            ""
        } else {
            "  (NOT IMPLEMENTED YET)"
        };
        println!("{mark} {:<20} {}{runs}", entry.name, entry.vendor);
        match &status.path {
            Some(path) => {
                let version = status.version.as_deref().unwrap_or("?");
                println!("    {} · version {version}", path.display());
            }
            None => println!("    not installed"),
        }
        if let Some(problem) = &status.problem {
            println!("    {problem}");
        }
        if let (true, Some(min)) = (status.old(), entry.min_version) {
            println!("    older than {min}, which the harness was checked with");
        }
        if let Some(host) = entry.inside {
            println!("    runs inside {host}");
        }
        match status.login {
            Some(true) => println!("    login saved"),
            Some(false) => println!("    no login: sign in on the Agents tab (harness tui)"),
            None => {}
        }
        println!("    plan: {}", entry.plan);
        if !entry.runs() {
            println!("    NOT IMPLEMENTED YET: the harness has no adapter for it, so roles");
            println!("    cannot use it. If you need it, ask the developer to implement it.");
        }
        if let Some((action, command)) = status.action() {
            let what = match action {
                catalog::Action::Install => "install",
                catalog::Action::Update => "update",
                catalog::Action::Remove => "remove",
            };
            match (command, status.needs_sudo(catalog::Action::Update)) {
                (Some(command), _) => println!("    {what}: {command}"),
                (None, Some(sudo)) => println!("    {what} (in a terminal): {sudo}"),
                (None, None) => println!("    {what}: see {}", entry.site),
            }
        }
        match (status.removal(), status.needs_sudo(catalog::Action::Remove)) {
            (Some(command), _) => println!("    remove: {command}"),
            (None, Some(sudo)) => println!("    remove (in a terminal): {sudo}"),
            (None, None) => {}
        }
    }
}
