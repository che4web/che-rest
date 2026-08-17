# План: удобство для LLM-агентов

Цель: сделать `che-rest` предсказуемым и machine-readable, чтобы LLM-агенты могли создавать,
настраивать и проверять приложения без чтения большого объёма исходного кода.

## Приоритет 1: документация проекта [x]

- Генерировать `AGENTS.md` через `startproject`.
- Описать канонический workflow:
  `startapp` -> `makemigrations` -> `migrate` -> запуск сервера.
- Документировать структуру app, viewsets, serializers, filters, permissions, WebSocket signals и internal channels.
- Использовать короткие проверенные snippets без конкурирующих вариантов.
- Добавить раздел conventions:
  - `author_id` назначается через `ViewSet::prepare_create`;
  - таблицы создаются только через `migrate`;
  - public WebSocket signals отделены от internal `AppChannels`;
  - REST lifecycle signals публикуются viewsets; ORM writes не превращаются в application events автоматически;
  - generated admin использует session cookies и `csrf_token`.

## Приоритет 2: JSON-инспекция [ ]

Добавить команду:

```bash
cargo run --bin manage -- inspect --format json
```

Команда должна выводить:

- installed apps;
- модели и поля;
- foreign keys;
- routes и permissions;
- OpenAPI paths;
- зарегистрированные WebSocket signals;
- состояние migrations;
- auth/session configuration.

JSON должен быть стабильным, без человекоориентированных сообщений в stdout. Диагностика может
выводиться в stderr.

## Приоритет 3: машинные contracts

- Сохранить OpenAPI как основной HTTP contract.
- Добавить описание WebSocket signals в machine-readable виде через
  `x-che-rest-signals` в OpenAPI.
- Для каждого signal описывать имя, authentication requirement и payload schema, когда она известна.
- Генерировать TypeScript types для signal names.

## Приоритет 4: идемпотентный CLI [ ]

Для команд добавить структурированный режим `--format json`:

- `makemigrations`: `created`, `unchanged`, `error`;
- `migrate`: применённые и пропущенные миграции;
- `generate-ts` и `generate-admin`: созданные, обновлённые и сохранённые файлы.

Команды должны быть безопасны для повторного запуска и явно сообщать, какие файлы изменились.

## Приоритет 5: канонический пример [x]

Развивать `examples/cli_fullstack` как основной vertical slice:

- CRUD;
- session auth;
- CSRF;
- WebSocket signal subscription;
- internal application channel;
- declared REST lifecycle signals;
- task author из текущего пользователя;
- TypeScript client;
- Vue admin.

Добавить e2e smoke test:

1. Создать или применить migrations.
2. Создать пользователя и выполнить session login.
3. Создать task через REST.
4. Подписаться на `tasks.created` через WebSocket.
5. Создать task через REST.
6. Проверить `author_id` и публикацию `tasks.created`.

Smoke test запускается командой:

```bash
cargo test --manifest-path examples/cli_fullstack/Cargo.toml --test smoke
```

## Приоритет 6: error cookbook

Документировать типовые ошибки в формате «ошибка -> причина -> проверка -> исправление»:

- `404 /api-session-auth/login/`: неверный Vite target или не установлен auth module;
- `CSRF validation failed`: отсутствует cookie или `X-CSRF-Token`;
- `no such table`: не выполнен `migrate`;
- `authentication credentials were not provided`: отсутствует session/token;
- migration/schema mismatch: нужно повторно выполнить `makemigrations`.

## Порядок реализации

1. Генерация `AGENTS.md` и улучшение canonical example.
2. `inspect --format json`.
3. WebSocket signal contract и TypeScript types.
4. JSON output для management commands.
5. E2E smoke tests.
6. Error cookbook и автоматическая проверка документации.
