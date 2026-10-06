//! The skills that come with the program, and the base each role always gets.

use crate::task::handoff::Role;

/// The built-in skills: name and text.
pub(super) const BUILT_IN: [(&str, &str); 12] = [
    ("common", include_str!("../../skills/common.md")),
    ("architect", include_str!("../../skills/architect.md")),
    ("developer", include_str!("../../skills/developer.md")),
    ("tester", include_str!("../../skills/tester.md")),
    ("security", include_str!("../../skills/security.md")),
    ("agent-claude", include_str!("../../skills/agent-claude.md")),
    ("agent-codex", include_str!("../../skills/agent-codex.md")),
    (
        "agent-antigravity",
        include_str!("../../skills/agent-antigravity.md"),
    ),
    ("agent-dsh", include_str!("../../skills/agent-dsh.md")),
    (
        "filesystem-attacks",
        include_str!("../../skills/filesystem-attacks.md"),
    ),
    (
        "crash-recovery",
        include_str!("../../skills/crash-recovery.md"),
    ),
    (
        "protocol-attacks",
        include_str!("../../skills/protocol-attacks.md"),
    ),
];

/// The text of the built-in skill `name`.
pub fn built_in(name: &str) -> Option<&'static str> {
    BUILT_IN
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

/// The names of all built-in skills.
pub fn built_in_names() -> impl Iterator<Item = &'static str> {
    BUILT_IN.iter().map(|(name, _)| *name)
}

/// The note for the agent a role runs on.
pub fn agent_note(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("agent-claude"),
        "codex" => Some("agent-codex"),
        "antigravity" => Some("agent-antigravity"),
        "dsh" => Some("agent-dsh"),
        _ => None,
    }
}

/// The base of a role on `agent`: always in its prompt, never chosen.
pub fn base_names(role: Role, agent: &str) -> Vec<&'static str> {
    // Each AI role's base skill has the role's own name; Lisa has none.
    let own = (role != Role::Human).then(|| role.as_str());
    ["common"]
        .into_iter()
        .chain(own)
        .chain(agent_note(agent))
        .collect()
}

/// Is `name` part of some role's base (not an optional skill)?
pub fn is_base(name: &str) -> bool {
    matches!(
        name,
        "common" | "architect" | "developer" | "tester" | "security"
    ) || name.starts_with("agent-")
}

/// A short fingerprint of a built-in text (FNV-1a), kept in a copy's header.
pub fn fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The built-in skill `name` as a project file to edit: its text with the
/// fingerprint in the header.
pub fn copy_of_built_in(name: &str) -> Option<String> {
    let text = built_in(name)?;
    let rest = text.strip_prefix("---\n")?;
    Some(format!("---\nbuiltin: {}\n{rest}", fingerprint(text)))
}
