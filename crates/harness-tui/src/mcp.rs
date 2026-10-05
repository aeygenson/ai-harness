//! The MCP tab's work: checking a server, signing in, searching the registry,
//! and saving servers and their secrets to `harness.toml`.

use std::sync::mpsc;

use harness_agents::credentials;
use harness_core::config::McpConfig;
use harness_core::config_edit;
use harness_core::mcp_tools::{self, Tool, ToolList};

use crate::mcp_tab;
use crate::roles_tab::RolesTab;
use crate::ui::Form;
use crate::{server_form, App, Purpose};

impl App {
    /// «Check»: the server starts in the background and is asked for its
    /// tools, with its secrets as a run would give them.
    pub(crate) fn check_mcp(&mut self) {
        let (Some(root), Some(mcp), Some(roles)) = (&self.project, &mut self.mcp, &self.roles)
        else {
            return;
        };
        if self.checking.is_some() {
            return;
        }
        let Some(name) = mcp.current(roles) else {
            return;
        };
        let Some(config) = roles.servers().get(&name).cloned() else {
            return;
        };
        let secrets = self.home.as_ref().map(|h| h.join("credentials"));
        let server = harness_core::mcp::server(&name, &config, |secret| {
            harness_agents::mcp_oauth::mcp_secret(secrets.as_deref()?, secret)
        });
        // A web server is reached through this same program.
        let server = server.map(|server| match std::env::current_exe() {
            Ok(harness) => harness_agents::launcher::with_bridge(vec![server.clone()], &harness)
                .pop()
                .unwrap_or(server),
            Err(_) => server,
        });
        let server = match server {
            Ok(server) => server,
            Err(error) => {
                self.message = Some((error.to_string(), true));
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let (checker, root) = (self.checker, root.clone());
        std::thread::spawn(move || {
            let _ = tx.send(checker(&server, &root));
        });
        mcp.checking = Some(name.clone());
        self.message = Some((self.tr.f("mcp.checking", &[("name", &name)]), false));
        self.checking = Some((name, config, rx));
    }

    /// The server answered: keep its tools and say how many.
    pub(crate) fn mcp_checked(
        &mut self,
        name: &str,
        server: &McpConfig,
        answer: Result<Vec<Tool>, String>,
    ) {
        if let Some(mcp) = &mut self.mcp {
            mcp.checking = None;
        }
        self.message = Some(match answer {
            Ok(tools) => {
                let count = tools.len();
                let list = ToolList {
                    server: name.to_string(),
                    fetched: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs()),
                    tools,
                };
                match &self.home {
                    Some(home) => match mcp_tools::save(home, server, &list) {
                        Ok(()) => (
                            self.tr
                                .f("mcp.checked", &[("name", &name), ("count", &count)]),
                            false,
                        ),
                        Err(error) => (error.to_string(), true),
                    },
                    None => ("HOME is not set".into(), true),
                }
            }
            Err(error) => (error, true),
        });
    }

    /// Signs in to the web server `name` in the background: the browser
    /// opens, and the answer comes in `tick`.
    fn start_sign_in(&mut self, name: &str) {
        let url = self
            .roles
            .as_ref()
            .and_then(|r| r.servers().get(name))
            .and_then(|s| s.url.clone());
        let (Some(url), Some(home), Some(mcp)) = (url, &self.home, &mut self.mcp) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (signer, dir, server) = (self.signer, home.join("credentials"), name.to_string());
        std::thread::spawn(move || {
            let _ = tx.send(signer(&dir, &server, url.trim()));
        });
        mcp.signing = Some(name.to_string());
        self.message = Some((self.tr.f("mcp.sign_in_started", &[("name", &name)]), false));
        self.signing = Some((name.to_string(), rx));
    }

    /// Asks the registry in the background; the catalog shows the answer.
    pub(crate) fn search_registry(&mut self, query: &str) {
        let Some(mcp) = &mut self.mcp else {
            return;
        };
        query.clone_into(&mut mcp.last_query);
        let Some(catalog) = &mut mcp.catalog else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (searcher, query) = (self.searcher, query.to_string());
        catalog.query.clone_from(&query);
        catalog.searching = true;
        catalog.error = None;
        std::thread::spawn(move || {
            let _ = tx.send(searcher(&query));
        });
        self.searching = Some(rx);
    }

    /// What the MCP tab asks for.
    pub(crate) fn mcp_action(&mut self, action: mcp_tab::Action) {
        use mcp_tab::Action as A;
        let tr = &self.tr;
        let servers = self.roles.as_ref().map(RolesTab::servers);
        self.form = match action {
            A::None => return,
            A::SignIn(name) => {
                self.start_sign_in(&name);
                return;
            }
            A::Unusable(why) => {
                self.message = Some((tr.f("mcp.cannot_use", &[("why", &why)]), true));
                return;
            }
            A::Search(query) => Some((
                Purpose::McpSearch,
                Form::new(
                    tr.t("mcp.search_title"),
                    tr.t("mcp.search_text"),
                    tr.t("mcp.search"),
                )
                .field(tr.t("mcp.search_field"), &query),
            )),
            A::Use(offer) => {
                // A name harness.toml does not use yet.
                let taken = |name: &str| servers.is_some_and(|s| s.contains_key(name));
                let name = std::iter::once(offer.name.clone())
                    .chain((2..).map(|n| format!("{}-{n}", offer.name)))
                    .find(|n| !taken(n))
                    .unwrap_or_default();
                let text = format!("{}\n{}", tr.t("mcp.from_catalog"), tr.t("mcp.form_text"));
                Some((
                    Purpose::McpServer(None),
                    server_form(tr, tr.t("mcp.new_title"), &text, &name, &offer.server),
                ))
            }
            A::New => Some((
                Purpose::McpServer(None),
                server_form(
                    tr,
                    tr.t("mcp.new_title"),
                    tr.t("mcp.form_text"),
                    "",
                    &McpConfig::default(),
                ),
            )),
            A::Edit(name) => {
                let server = servers.and_then(|s| s.get(&name));
                let old = server.map(|_| name.clone());
                let title = tr.f("mcp.edit_title", &[("name", &name)]);
                let empty = McpConfig::default();
                Some((
                    Purpose::McpServer(old),
                    server_form(
                        tr,
                        &title,
                        tr.t("mcp.form_text"),
                        &name,
                        server.unwrap_or(&empty),
                    ),
                ))
            }
            A::Remove(name) => {
                let text = tr.f("mcp.remove_text", &[("name", &name)]);
                Some((
                    Purpose::RemoveMcp(name),
                    Form::new(tr.t("mcp.remove_title"), &text, tr.t("mcp.remove")),
                ))
            }
            A::Secret(name) => Some((
                Purpose::Secret,
                Form::new(
                    tr.t("mcp.secret_title"),
                    tr.t("mcp.secret_text"),
                    tr.t("mcp.ok"),
                )
                .field(tr.t("mcp.secret_name"), &name)
                .secret(tr.t("mcp.secret_value")),
            )),
        };
        // The value is typed into the second field.
        if let Some((Purpose::Secret, form)) = &mut self.form {
            form.focus = 1;
        }
    }

    /// OK in the server form.
    pub(crate) fn save_mcp(&mut self, old: Option<String>, form: &Form) -> Result<(), String> {
        let name = form.value(0).to_string();
        let server = mcp_tab::server_from(form.value(1), form.value(2), form.value(3))?;
        harness_core::mcp::check_server(&name, &server).map_err(|e| e.to_string())?;
        self.save_settings(|text| {
            config_edit::set_mcp(text, old.as_deref(), &name, &server).map_err(|e| e.to_string())
        })?;
        if let (Some(mcp), Some(roles)) = (&mut self.mcp, &self.roles) {
            // A server from the catalog is now in the list.
            mcp.catalog = None;
            mcp.select_named(&name, roles);
        }
        self.message = Some((self.tr.f("mcp.saved", &[("name", &name)]), false));
        Ok(())
    }

    /// Removes an MCP server from `harness.toml`.
    pub(crate) fn remove_mcp(&mut self, name: &str) -> Result<(), String> {
        self.save_settings(|text| config_edit::remove_mcp(text, name).map_err(|e| e.to_string()))?;
        self.message = Some((self.tr.f("mcp.removed", &[("name", &name)]), false));
        Ok(())
    }

    /// OK in the secret form: the value goes to a private file, nowhere else.
    pub(crate) fn save_secret(&mut self, form: &Form) -> Result<(), String> {
        let name = form.value(0).to_string();
        if !harness_core::mcp::is_simple_name(&name) {
            return Err(self.tr.f("mcp.bad_secret_name", &[("name", &name)]));
        }
        let value = form.value(1);
        if value.is_empty() {
            return Err(self.tr.t("mcp.empty_secret").to_string());
        }
        let Some(home) = &self.home else {
            return Err("HOME is not set".into());
        };
        credentials::save_secret(
            &home.join("credentials"),
            &name,
            &credentials::Secret::new(value),
        )
        .map_err(|e| e.to_string())?;
        if let Some(mcp) = &mut self.mcp {
            mcp.reload();
        }
        self.message = Some((self.tr.f("mcp.secret_saved_as", &[("name", &name)]), false));
        Ok(())
    }
}
