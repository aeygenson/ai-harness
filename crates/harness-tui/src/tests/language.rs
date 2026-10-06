//! Tests of the language switch, which changes every tab at once.

use super::*;

#[test]
fn the_language_switches_and_is_remembered() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    assert!(screen(&mut app).contains("1 Tasks"));

    click(&mut app, " EN ");
    let text = screen(&mut app);
    for part in [
        "1 Задачи │ 2 Роли",
        " + Новый проект   RU ",
        "q выход",
        "раунд 1 · tester",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // The choice is kept for the next start.
    let mut again = env.app(&root);
    assert!(screen(&mut again).contains("1 Задачи"));

    // The button in the top bar starts a new project from any tab.
    click(&mut again, "+ Новый проект");
    let text = screen(&mut again);
    assert!(text.contains("Папка для нового проекта"), "{text}");
    assert!(text.contains(" Выбрать эту папку "), "{text}");
    key(&mut again, KeyCode::Esc);

    key(&mut again, KeyCode::Char('L'));
    assert!(screen(&mut again).contains("1 Tasks"));
}
