# Portable push application

Available in the `che-rest 0.3.3` source release; minimum Rust version is 1.88.

Enable the optional `push` feature. It adds authenticated device registration, compiled migrations, a durable SQLite outbox, checkpoint storage, retries/leases and an FCM HTTP v1 sender. It has no dependency on application models, model-sync journals, UUID domain identifiers, Android package names or application environment-variable prefixes.

```toml
che-rest = { path = "../che-rest", features = ["push"] }
```

This implementation is in the updated source checkout. Until a release containing the feature is published, use its path or a Git revision containing these changes.

## Installation

Implement `PushSource` for the application's event policy, then install the module alongside auth:

```rust,ignore
use std::sync::Arc;
use che_rest::{InstalledApps, push::{self, PushConfig}};

let source = Arc::new(MyNotificationSource);
let config = PushConfig {
    enabled: true,
    package_name: Some("org.example.billing".into()),
    project_id: None, // otherwise override project_id in service-account JSON
};
let apps = InstalledApps::new()
    .add(che_rest::auth::module())
    .add(push::module(config.clone(), source.clone()));
// Give the same app registry to Management and Server.
// Apply `manage migrate` explicitly before startup.
let router = che_rest::Server::new(state.clone()).install(apps).build().await?;
let _worker = push::start(state.database().clone(), config, source).await?;
```

The module is named `notifications`. `AppModule::migrations()` supplies its frozen `notifications:0001_initial` history. Management automatically collects installed histories. Projects using a legacy explicit migration registry must additionally include `push::migrations::all()` there and use `Management::without_library_migrations(&["notifications"])` to avoid registering that same history twice. Never regenerate the library's initial migration in a consumer or create its tables during startup.

Tables: `push_devices`, `push_cursors`, `push_deliveries`. These are server-only infrastructure, without domain FKs or sync replication. Domain data can use integer IDs, UUIDs or business strings; an event's `target_id` is a string.

`push::start` checks checkpoint-storage availability, loads service-account credentials and starts the five-second worker only when enabled. It does not authenticate against Firebase during startup or prove live IAM/network access. `PushConfig::default()` disables it and does not load credentials. Set `GOOGLE_APPLICATION_CREDENTIALS` to the path of the server-only service-account JSON; enable FCM HTTP v1 for its Firebase project. Android's `google-services.json` belongs to the consumer's build, not to this module. Keep private keys outside the repository and public files.

## Application contract

`PushSource` has three transaction-bound methods:

- `initialize(tx)` creates missing source checkpoints at current heads when registering a device, so older domain activity does not become a notification backlog. Preserve existing checkpoints.
- `capture(tx)` reads a bounded batch of committed facts, returns `Vec<PushEvent>` and advances checkpoints. Event queue insertion and checkpoint updates commit together. Use the supplied configured transaction; do not use a separate connection, network I/O or an in-memory queue here.
- `relevant(tx, delivery)` rechecks domain existence and current recipients before a queued delivery is claimed. Enforce project-specific access here.

`PushEvent` supplies a stable key, recipients as authenticated `User.id` integers, `target_type`, string `target_id`, `kind`, `title`, `body`, string-to-string `data` and the original event timestamp. Keys must identify immutable source events and remain stable after restart. Retries must not regenerate creation times, silently skip facts or reset checkpoints. If using a model-sync journal, persist/check its history alongside position in `PushCursor`.

The transport is independent of sync: the tests exercise a billing source with target `invoice/INV-42`, without tasks, employees, sync metadata or domain tables. The source can instead use a transactional application outbox or other durable facts. Do not call external APIs inside capture.

Device filtering respects recipients, registration time, enabled status and binding generation. The queue deduplicates a JSON tuple of event key, target type, target ID, installation ID and binding generation. A serialized event content budget of 2500 bytes leaves room for transport metadata. Oversized events fail capture and roll back checkpoint changes.

## Client contract

Authenticated endpoints, under the configured API prefix:

- `GET /push/status/` → `{ "enabled": true }`.
- `POST /push/devices/register/` with `{ "id": "installation UUID", "token": "FCM token", "server_key": "64 hex SHA-256 of canonical server URL" }`.
- `POST /push/devices/unregister/` with `{ "id": "installation UUID" }`.

Ownership comes from authentication, never a submitted user ID. Only the current owner's unregister request disables an installation. Rebinding bumps the generation and cancels old queued jobs. IDs must be random per installation; retain them across token rotations, disable Android backup and keep tokens native/private. Registration retries with identical settings are idempotent.

FCM data-only messages contain `event_id`, `target_type`, `target_id`, `user_id`, `server_key`, `kind`, `title`, `body` plus application `data`. Reserved fields are overwritten by server metadata. The optional package restriction comes from `PushConfig`; no Tacit identifiers are embedded. The application chooses which content is suitable for FCM.

Each consumer implements its native message handler and routing. Check the active account/server before showing anything, serialize that check with logout, request Android permission explicitly, and retain taps without replacing unsaved editors. Disable local display before attempting network unregister. Refresh registration after FCM token rotation. A Vue settings screen, native SDK integration and deep-link destination remain application UI responsibilities.

Delivery is at least once, bounded by retries/expiry and Android/FCM conditions. HTTP 429/5xx/OAuth/network failures retry; only explicit FCM `UNREGISTERED` disables a token. Expired leases recover after restart. Use the stable event ID to replace duplicate native notifications. `PushSender` and `delivery::drain` allow custom transports/testing; the bundled startup helper uses FCM.

## Lifecycle and retries

The module's `init` phase only declares routes and schema. `AppModule::start` does
not automatically launch the push worker. Call `push::start` explicitly after
migrations and router build, with the same configuration/source and database.
For sync consumers this must be the configured sync database handle.

The bundled worker processes bounded captures every five seconds. It claims at
most 50 due deliveries per drain, with a 60-second lease and a unique lease token.
Retries respect numeric Retry-After or use exponential backoff; after 12 attempts
a job becomes `failed`, and jobs older than two days are cancelled when processed.
FCM retention is one day. Terminal rows are retained; no automatic terminal-row
cleanup or failed-job resubmission is provided.

`delivered` means FCM accepted the send request. It does not confirm device display,
reading or application synchronization. Delivery can repeat after an interrupted
send. The stable event ID lets a native client replace a duplicate notification.
A late UNREGISTERED result is applied only to the same token/binding generation.

`GET /push/status/` reports configured enablement, not transport health. Queue
states and sanitized `last_error` codes are available through trusted server-side
ORM/SQLite inspection; the module exposes no public queue/device listing API.
Do not log private keys, full FCM tokens or response bodies while diagnosing it.

## Verification

From the che-rest checkout:

```sh
cargo fmt --check
cargo test --all-features
cargo clippy --all-features --all-targets -- -D warnings
cargo +1.88.0 check --features push --lib
```

HTTP tests bind ephemeral loopback ports. Default-feature checks alone exclude
push. The independent test source emits an invoice with a string domain ID; tests
cover compiled-schema freshness/replay, deduplication through checkpoint progress,
queue delivery and atomic rollback of an oversized event. Consumer migration tests
must additionally prove preservation of their previous registrations, queued IDs,
attempts and source checkpoints.

These checks use fake senders. Test real FCM after installing the consumer's
configured native client, including permission refusal, process closure, account
switch/logout, token rotation and network loss. The bundled transport is for
Android FCM; native UI/SDK integration is not supplied by che-rest.
