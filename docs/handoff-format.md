# Формат передачи работы между ролями (handoff)

Этот документ описывает, как роли харнесса (Architect, Developer, Tester, Security)
передают работу друг другу. Каждая роль, закончив свой шаг, пишет **два файла**:

| Файл | Для кого | Что внутри |
|------|----------|------------|
| `notes.md` | для человека (Лиза) | короткие заметки своими словами: что сделано, что смущает |
| `handoff.json` | для харнесса | тот же результат в строгом формате, по которому харнесс решает, что делать дальше |

`notes.md` пишется свободно. `handoff.json` обязан точно соответствовать формату ниже:
если AI напишет что-то лишнее или неизвестное значение, харнесс файл не примет.

## Поля `handoff.json`

| Поле | Тип | Обязательное | Описание |
|------|-----|--------------|----------|
| `schema_version` | число | да | Версия формата. Сейчас `1`. Нужна, чтобы потом менять формат, не ломая старые файлы. |
| `task_id` | строка | да | Какая это задача, например `"task-001"`. |
| `round` | число | да | Какой это круг по циклу (1, 2, 3...). Растёт при каждом возврате назад. |
| `role` | роль | да | Кто написал файл: `architect`, `developer`, `tester`, `security` или `human` (решение Лизы). |
| `verdict` | вердикт | да | Решение роли: `approved`, `rejected`, `needs_human`. |
| `next_role` | куда дальше | да | Куда роль **предлагает** отправить работу: `architect`, `developer`, `tester`, `security`, `human`, `done`. |
| `summary` | строка | да | Итог в одном-двух предложениях. |
| `skills_used` | список строк | нет | Какие навыки (skills) роль использовала. |
| `files` | список | нет | Какие файлы роль создала, изменила, удалила или прочитала. |
| `issues` | список | нет | Найденные проблемы. Обязательно заполнять при `rejected`, чтобы следующая роль знала, что исправлять. |

### Значения `verdict`

- `approved` — работа принята (для Tester и Security) или закончена (для Architect и Developer).
- `rejected` — работа не принята, её нужно вернуть назад; причины в `issues`.
- `needs_human` — роль не может решить сама и просит Лизу посмотреть.

Для `human` (Лиза): `approved` — дизайн утверждён, `rejected` — вернуть архитектору.

### Элемент списка `files`

```json
{ "path": "src/parser.rs", "action": "modified" }
```

`action` — одно из: `created`, `modified`, `deleted`, `read`.

### Элемент списка `issues`

```json
{ "severity": "high", "location": "src/parser.rs:42", "description": "Паника на пустом вводе." }
```

- `severity` — одно из: `low`, `medium`, `high`, `critical`.
- `location` — где проблема (файл и строка, или раздел документа). Можно не указывать.
- `description` — что не так.

## Правила маршрутизации

Роль **предлагает** `next_role`, а харнесс **проверяет**, разрешён ли такой переход.
Если переход не из таблицы, харнесс останавливается и показывает ситуацию Лизе.

| Кто | Куда можно отправить |
|-----|----------------------|
| Architect | `human` — всегда на утверждение Лизе |
| Лиза (human) | любая роль или `done`: Лиза главная. Обычно `developer` — дизайн утверждён; `architect` — переделать дизайн |
| Developer | `tester` — код готов; `architect` — дизайн неполный или непонятный |
| Tester | `security` — тесты прошли; `developer` — баг; `architect` — проблема большая, в самом дизайне |
| Security | `done` — всё прошло; `developer` — исправить код; `architect` — проблема в дизайне |
| Любая роль | `human` вместе с `verdict: "needs_human"` — остановиться и спросить Лизу |

Дополнительные проверки харнесса:

- `role` в файле должен совпадать с ролью, которая сейчас работает.
- `rejected` без единого `issues` — ошибка (непонятно, что исправлять).
- Предел кругов (`round`) по умолчанию **5**, чтобы цикл не шёл бесконечно. При достижении предела работа останавливается и уходит к Лизе.

## Где лежат файлы

Реализовано в `crates/harness-core/src/store.rs` (подробнее — раздел 8 в [design.md](design.md)):

```
runs/
  task-001/
    task.md
    state.json
    round-01/
      01-architect/  notes.md  handoff.json
      02-human/      notes.md  handoff.json
      03-developer/  notes.md  handoff.json
      04-tester/     notes.md  handoff.json
    round-02/
      01-developer/  ...
```

## Примеры

### Architect: дизайн готов, отправляет Лизе на утверждение

```json
{
  "schema_version": 1,
  "task_id": "task-001",
  "round": 1,
  "role": "architect",
  "verdict": "approved",
  "next_role": "human",
  "summary": "Дизайн парсера конфигурации готов: модули, форматы, обработка ошибок.",
  "skills_used": ["write-design-doc"],
  "files": [{ "path": "docs/design/config-parser.md", "action": "created" }],
  "issues": []
}
```

### Лиза (human): дизайн утверждён, отправляет разработчику

Решение Лизы записывается в том же формате, поэтому вся история задачи хранится одинаково.
Когда появится GUI, этот файл будет создаваться кнопкой «Утвердить» или «Вернуть».

```json
{
  "schema_version": 1,
  "task_id": "task-001",
  "round": 1,
  "role": "human",
  "verdict": "approved",
  "next_role": "developer",
  "summary": "Дизайн утверждён. Неизвестные ключи считать ошибкой.",
  "skills_used": [],
  "files": [{ "path": "docs/design/config-parser.md", "action": "read" }],
  "issues": []
}
```

### Developer: код написан, отправляет тестировщику

```json
{
  "schema_version": 1,
  "task_id": "task-001",
  "round": 1,
  "role": "developer",
  "verdict": "approved",
  "next_role": "tester",
  "summary": "Реализован парсер по дизайну, добавлены базовые тесты.",
  "skills_used": ["rust-error-handling"],
  "files": [
    { "path": "src/parser.rs", "action": "created" },
    { "path": "docs/design/config-parser.md", "action": "read" }
  ],
  "issues": []
}
```

### Developer: дизайна не хватает, возвращает архитектору

```json
{
  "schema_version": 1,
  "task_id": "task-001",
  "round": 1,
  "role": "developer",
  "verdict": "rejected",
  "next_role": "architect",
  "summary": "Не могу продолжать: в дизайне не описано, что делать с неизвестными ключами.",
  "skills_used": [],
  "files": [{ "path": "docs/design/config-parser.md", "action": "read" }],
  "issues": [
    {
      "severity": "medium",
      "location": "docs/design/config-parser.md, раздел «Ошибки»",
      "description": "Нет правила для неизвестных ключей: игнорировать или считать ошибкой?"
    }
  ]
}
```

### Tester: нашёл баг, возвращает разработчику

```json
{
  "schema_version": 1,
  "task_id": "task-001",
  "round": 2,
  "role": "tester",
  "verdict": "rejected",
  "next_role": "developer",
  "summary": "2 из 5 тестов падают на пустом вводе.",
  "skills_used": ["write-unit-tests"],
  "files": [{ "path": "src/parser.rs", "action": "read" }],
  "issues": [
    {
      "severity": "high",
      "location": "src/parser.rs:42",
      "description": "Паника на пустом вводе."
    }
  ]
}
```

### Tester: всё прошло, отправляет в Security

```json
{
  "schema_version": 1,
  "task_id": "task-001",
  "round": 3,
  "role": "tester",
  "verdict": "approved",
  "next_role": "security",
  "summary": "Все 7 тестов проходят, включая пустой и очень большой ввод.",
  "skills_used": ["write-unit-tests"],
  "files": [{ "path": "tests/parser_tests.rs", "action": "created" }],
  "issues": []
}
```

### Security: всё в порядке, задача закончена

```json
{
  "schema_version": 1,
  "task_id": "task-001",
  "round": 3,
  "role": "security",
  "verdict": "approved",
  "next_role": "done",
  "summary": "Секретов в коде нет, ввод ограничен по размеру.",
  "skills_used": ["check-leaked-secrets"],
  "files": [{ "path": "src/parser.rs", "action": "read" }],
  "issues": []
}
```

### Любая роль: нужен человек

```json
{
  "schema_version": 1,
  "task_id": "task-001",
  "round": 2,
  "role": "security",
  "verdict": "needs_human",
  "next_role": "human",
  "summary": "Парсер читает файлы по любому пути. Нужно решение: ограничивать ли папкой проекта?",
  "skills_used": ["path-traversal-check"],
  "files": [{ "path": "src/parser.rs", "action": "read" }],
  "issues": [
    {
      "severity": "medium",
      "location": "src/parser.rs:10",
      "description": "Путь к файлу берётся из ввода без проверки."
    }
  ]
}
```

## Принятые решения

- Решение Лизы записывается как `handoff.json` с ролью `human` (25.09.2026).
- Предел кругов по умолчанию — 5 (25.09.2026).
