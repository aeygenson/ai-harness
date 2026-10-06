//! The right side of the MCP tab: everything about the selected server.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use harness_core::config::{AgentKind, McpConfig};
use harness_core::mcp::{self, SECRET_PREFIX};

use super::tab::{is_oauth, McpTab};
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;
use crate::ui::i18n::I18n;
use crate::ui::theme;

impl McpTab {
    /// The address and headers of a web server.
    fn web_details(
        &self,
        name: &str,
        server: &McpConfig,
        url: &str,
        tr: &I18n,
        lines: &mut Vec<Line<'static>>,
    ) {
        let dim = theme::dim();
        let red = theme::bad();
        let green = theme::ok();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.address")), bold),
            Span::raw(url.to_string()),
        ]));
        lines.push(Line::styled(tr.t("mcp.address_hint").to_string(), dim));
        lines.push(Line::default());
        if is_oauth(server) {
            let state = if self.signing.as_deref() == Some(name) {
                Span::styled(tr.t("mcp.signing_in").to_string(), dim)
            } else if self.signed_in(name, server) {
                Span::styled(tr.t("mcp.signed_in").to_string(), green)
            } else {
                Span::styled(tr.t("mcp.not_signed_in").to_string(), red)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", tr.t("mcp.sign_in_label")), bold),
                state,
            ]));
            lines.push(Line::styled(tr.t("mcp.sign_in_hint").to_string(), dim));
            lines.push(Line::default());
        }
        if server.headers.is_empty() {
            lines.push(Line::styled(tr.t("mcp.no_headers").to_string(), dim));
        } else {
            lines.push(Line::styled(tr.t("mcp.headers").to_string(), bold));
            for (header, value) in &server.headers {
                let mut spans = vec![Span::raw(format!("  {header}: {value}"))];
                if let Some((_, secret)) = value.split_once(SECRET_PREFIX) {
                    let secret = secret.trim();
                    spans.push(if self.secrets.iter().any(|s| s == secret) {
                        Span::styled(format!("  {}", tr.t("mcp.secret_saved")), green)
                    } else {
                        Span::styled(
                            format!("  {}", tr.f("mcp.secret_missing", &[("name", &secret)])),
                            red,
                        )
                    });
                }
                lines.push(Line::from(spans));
            }
        }
    }

    /// What the details show about the server `name`.
    pub(super) fn details(&self, roles: &RolesTab, name: &str, tr: &I18n) -> Vec<Line<'static>> {
        let dim = theme::dim();
        let red = theme::bad();
        let green = theme::ok();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let mut lines = Vec::new();
        let Some(server) = roles.servers().get(name) else {
            lines.push(Line::styled(
                tr.f("mcp.not_described", &[("name", &name)]),
                red,
            ));
            return lines;
        };
        if let Some(url) = &server.url {
            self.web_details(name, server, url, tr, &mut lines);
        } else {
            self.command_details(server, tr, &mut lines);
        }

        // Which agents can start it; the role's own agent in bold.
        lines.push(Line::default());
        let agent = roles.settings(self.role()).map(|s| s.agent);
        let mut spans = vec![Span::styled(format!("{} ", tr.t("mcp.agents")), bold)];
        for (index, each) in AgentKind::ALL.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(" · ", dim));
            }
            let (mark, look) = if mcp::agent_runs(each, server) {
                ("✓", green)
            } else {
                ("✗", red)
            };
            let style = if agent == Some(each) {
                Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new()
            };
            spans.push(Span::styled(each.to_string(), style));
            spans.push(Span::styled(format!(" {mark}"), look));
        }
        lines.push(Line::from(spans));
        lines.push(Line::styled(tr.t("mcp.agents_hint").to_string(), dim));

        // The tools, as the server said when it was checked last.
        self.tools_details(name, server, tr, &mut lines);

        lines.push(Line::default());
        let users: Vec<&str> = ROLES
            .iter()
            .filter(|r| {
                roles
                    .settings(**r)
                    .is_some_and(|s| s.mcp.iter().any(|n| n == name))
            })
            .map(|r| r.as_str())
            .collect();
        let users = if users.is_empty() {
            tr.t("mcp.no_roles").to_string()
        } else {
            users.join(", ")
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.roles")), bold),
            Span::raw(users),
        ]));
        lines
    }

    /// The command and the variables of a server started on this computer.
    fn command_details(&self, server: &McpConfig, tr: &I18n, lines: &mut Vec<Line<'static>>) {
        let dim = theme::dim();
        let red = theme::bad();
        let green = theme::ok();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let command = server
            .command
            .iter()
            .chain(&server.args)
            .map(|part| {
                if part.contains(char::is_whitespace) || part.is_empty() {
                    format!("{part:?}")
                } else {
                    part.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.command")), bold),
            Span::raw(command),
        ]));
        lines.push(Line::default());
        if server.env.is_empty() {
            lines.push(Line::styled(tr.t("mcp.no_variables").to_string(), dim));
        } else {
            lines.push(Line::styled(tr.t("mcp.variables").to_string(), bold));
            for (variable, value) in &server.env {
                let mut spans = vec![Span::raw(format!("  {variable} = {value}"))];
                if let Some(secret) = value.strip_prefix(SECRET_PREFIX) {
                    let secret = secret.trim();
                    spans.push(if self.secrets.iter().any(|s| s == secret) {
                        Span::styled(format!("  {}", tr.t("mcp.secret_saved")), green)
                    } else {
                        Span::styled(
                            format!("  {}", tr.f("mcp.secret_missing", &[("name", &secret)])),
                            red,
                        )
                    });
                }
                lines.push(Line::from(spans));
            }
        }
    }

    /// The server's tools, as it said when it was checked last.
    fn tools_details(
        &self,
        name: &str,
        server: &McpConfig,
        tr: &I18n,
        lines: &mut Vec<Line<'static>>,
    ) {
        let dim = theme::dim();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        lines.push(Line::default());
        let saved = self
            .home
            .as_deref()
            .and_then(|home| mcp::tools::load(home, name, server));
        match (&self.checking, saved) {
            (Some(checking), _) if checking == name => {
                lines.push(Line::styled(tr.t("mcp.tools_checking").to_string(), dim));
            }
            (_, Some(list)) => {
                lines.push(Line::styled(
                    tr.f("mcp.tools", &[("count", &list.tools.len())]),
                    bold,
                ));
                for tool in list.tools {
                    let mut spans = vec![Span::raw(format!("  {}", tool.name))];
                    if let Some(description) = tool.description {
                        spans.push(Span::styled(format!("  {description}"), dim));
                    }
                    lines.push(Line::from(spans));
                }
            }
            (_, None) => {
                lines.push(Line::styled(tr.t("mcp.tools_unknown").to_string(), dim));
            }
        }
    }
}
