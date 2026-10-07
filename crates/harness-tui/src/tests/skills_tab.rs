//! Tests of the Skills tab: choosing, editing, adding and restoring skills.

use super::*;

/// The editor was "closed": what `event_loop` does after the editor returns.
fn close_editor(app: &mut App) -> EditJob {
    let job = app.edit.take().expect("a skill to edit");
    app.finish_edit(&job, Ok(()));
    job
}

fn last_commit(root: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--format=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn the_skills_tab_shows_each_roles_base_and_the_skills_to_choose() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "3 Skills");
    let text = screen(&mut app);
    for part in [
        " architect ",
        "Skills of the architect",
        "Always in the prompt",
        "common",
        "● agent-claude",
        "○ agent-codex",
        "○ agent-antigravity",
        "Optional · click [ ] → [x] → [■]",
        "[ ] crash-recovery",
        "built-in",
        "common · built-in",
        " Edit in editor ",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // Another role shows its own skill; the selected skill stays.
    click(&mut app, " tester ");
    let text = screen(&mut app);
    assert!(text.contains("Skills of the tester"), "{text}");
    assert!(text.contains("│  tester "), "{text}");
    assert!(!text.contains("│  architect "), "{text}");
    assert!(text.contains("▶ common"), "{text}");
    // Paragraphs are joined and wrapped at the panel's width.
    assert!(
        text.contains("redesign what the design already decided"),
        "{text}"
    );
    // The note of an agent no role uses can be read and edited too.
    click(&mut app, "○ agent-codex");
    let text = screen(&mut app);
    assert!(text.contains("agent-codex · built-in"), "{text}");
    assert!(text.contains("Not in the tester's prompt"), "{text}");
    // A click on a skill shows its text.
    click(&mut app, "protocol-attacks");
    assert!(screen(&mut app).contains("protocol-attacks · built-in"));
}

#[test]
fn a_click_on_the_mark_of_a_skill_chooses_it_for_the_role() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "3 Skills");
    click(&mut app, " developer ");

    // Not used -> read when needed -> always in the prompt.
    click(&mut app, "[ ] crash-recovery");
    let text = screen(&mut app);
    assert!(text.contains("▶ [x] crash-recovery"), "{text}");
    assert!(text.contains("Changes not saved"), "{text}");
    click(&mut app, "[x] crash-recovery");
    assert!(screen(&mut app).contains("[■] crash-recovery"));

    // The Roles tab shows the same change, and «Save» keeps it.
    click(&mut app, " Save ");
    let developer = &config(&root).roles[&Role::Developer];
    assert!(developer
        .always_skills
        .contains(&"crash-recovery".to_string()));
    assert!(!developer.skills.contains(&"crash-recovery".to_string()));

    // Space goes on round the marks; «Undo changes» brings back the saved one.
    key(&mut app, KeyCode::Char(' '));
    assert!(screen(&mut app).contains("▶ [ ] crash-recovery"));
    key(&mut app, KeyCode::Char('u'));
    assert!(screen(&mut app).contains("▶ [■] crash-recovery"));

    // A click on the name only selects the skill.
    click(&mut app, "protocol-attacks");
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] protocol-attacks"), "{text}");
    assert!(text.contains("[■] crash-recovery"), "{text}");
}

#[test]
fn a_built_in_skill_is_copied_edited_committed_and_restored() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let file = root.join(".harness/skills/developer.md");
    let mut app = env.app(&root);
    key(&mut app, KeyCode::Char('3'));
    click(&mut app, " developer ");
    click(&mut app, "│  developer ");

    // Nothing changed in the editor: the copy goes away again.
    key(&mut app, KeyCode::Char('e'));
    assert!(file.is_file());
    assert!(fs::read_to_string(&file).unwrap().contains("builtin: "));
    close_editor(&mut app);
    assert!(!file.exists());
    assert!(screen(&mut app).contains("Nothing changed"));

    // A change is committed and the skill shows as changed.
    key(&mut app, KeyCode::Char('e'));
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{text}\nAlso run cargo fmt.\n")).unwrap();
    close_editor(&mut app);
    let text = screen(&mut app);
    assert!(
        text.contains("Skill developer saved and committed"),
        "{text}"
    );
    assert!(text.contains("developer · changed"), "{text}");
    assert_eq!(last_commit(&root), "harness: skill developer");

    // The developer's prompt gets the changed text.
    let skills = harness_core::skills::Skills::load(&root.join(".harness"), &config(&root))
        .unwrap()
        .for_role(Role::Developer);
    assert!(skills
        .base
        .iter()
        .any(|s| s.body.contains("Also run cargo fmt.")));

    // Restore asks first, then deletes the copy.
    click(&mut app, " Restore built-in ");
    let text = screen(&mut app);
    assert!(
        text.contains("Delete this project's copy of developer?"),
        "{text}"
    );
    key(&mut app, KeyCode::Enter);
    assert!(!file.exists());
    let text = screen(&mut app);
    assert!(text.contains("Skill developer is built-in again"), "{text}");
    assert!(text.contains("developer · built-in"), "{text}");
    assert_eq!(
        last_commit(&root),
        "harness: skill developer is built-in again"
    );
}

#[test]
fn a_new_skill_gets_a_file_and_can_be_chosen_on_roles() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    key(&mut app, KeyCode::Char('3'));
    click(&mut app, " New skill ");
    fill(&mut app, "Bad Name");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "Errors with thiserror.");
    key(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("is not allowed"));
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "crash-recovery");
    key(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("There is already a skill named crash-recovery"));
    fill(&mut app, "rust-errors");
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none());

    let file = root.join(".harness/skills/rust-errors.md");
    let job = close_editor(&mut app);
    assert_eq!(job.path, file);
    assert!(fs::read_to_string(&file)
        .unwrap()
        .starts_with("---\ndescription: Errors with thiserror.\n---\n"));
    assert_eq!(last_commit(&root), "harness: skill rust-errors");
    let text = screen(&mut app);
    assert!(text.contains("[ ] rust-errors"), "{text}");
    assert!(text.contains("rust-errors · own"), "{text}");
    // An own skill has nothing to restore.
    key(&mut app, KeyCode::Delete);
    assert!(app.form.is_none());

    click(&mut app, "2 Roles");
    click(&mut app, "developer  claude");
    assert!(screen(&mut app).contains("[ ] rust-errors"));
}
