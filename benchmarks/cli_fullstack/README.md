# cli_fullstack read benchmark

Compares the current `cli_fullstack` task list/retrieve endpoints with FastAPI using
direct `sqlite3`, FastAPI using SQLAlchemy ORM, and Django ORM. Every server reads
the same SQLite database and returns the same `TaskSerializer` JSON, including
nested `author`, nullable `assignee_id`, `status`, and timestamps. ORM servers load
the author eagerly, as the REST ViewSet does.

Requirements: Rust/Cargo, Python 3.11+, `uv`, and Apache Bench (`ab`). Atlas is not
used. Run commands from the repository root.

## Build and prepare

```bash
cargo build --release --manifest-path examples/cli_fullstack/Cargo.toml --bins
python3 benchmarks/cli_fullstack/prepare.py --rows 10000
```

The separate `benchmark_schema` binary applies the framework's compiled auth
migrations and a disposable fixture migration derived from the current Rust
`Task` schema. This is not production migration history: application startup is
unchanged and never creates model tables. Rebuild the binaries after model changes.

Preparation seeds one user and 10,000 tasks, covering all three status values and
both null/non-null assignees. It refuses to replace an existing database unless
`--force` is supplied. A new seed is built in a staging directory and replaces
the destination only after successful migration and insertion. Stop all database
users before replacement. The default is `benchmarks/cli_fullstack/benchmark.sqlite`;
the application's `db.sqlite` is not used.

```bash
python3 benchmarks/cli_fullstack/prepare.py --rows 10000 --force
```

Both scripts accept `--database PATH` for a separate fixture file. Rust receives
an isolated temporary config pointing at that file; no application config is edited.
Use plain `sqlite://PATH` URLs with the current ORM backend, not SQLx-style
`?mode=rwc` query parameters.

## Validate

```bash
python3 benchmarks/cli_fullstack/run.py --parity
python3 -m unittest discover -s benchmarks/cli_fullstack -p 'test_*.py'
```

Parity compares parsed JSON from all four servers for ordered lists, filtered
lists, a paginated list, an empty result, and detail retrieval. List responses
exercise enum values and nullable relations. `--parity` validates only; every
normal load run also performs these checks before measuring anything.

For fewer than 500 seed rows, choose an existing ID with `--retrieve-id`, e.g. `50`.

## Run

```bash
python3 benchmarks/cli_fullstack/run.py --requests 10000 --concurrency 50 --workers 4
```

The read-only scenarios are:

- `GET /api/tasks/?limit=50&ordering=-created_at`
- `GET /api/tasks/?limit=50&name__contains=task`
- `GET /api/tasks/500/`

Servers run **one at a time** on loopback ports 3101–3104. Each scenario has a
warmup (200 requests by default, configurable with `--warmup`) followed by Apache
Bench with HTTP keep-alive. Python uses four Uvicorn worker processes by default;
Rust uses its Tokio multithread runtime and a four-connection database pool.
These are different concurrency models, not an equal-CPU allocation.

The runner prints requests/sec, mean request latency (`concurrency / throughput`,
not the across-concurrency time reported by `ab`), p95 latency, failed requests,
and non-2xx responses. Any request failures or non-2xx responses invalidate the
run. Readiness errors include the HTTP response and server log instead of hiding
broken endpoints behind a startup timeout. Servers are stopped and reaped on exit.

Results and workload settings are saved to `benchmarks/cli_fullstack/results.json`
(override with `--output PATH`). Timings describe a single local run; repeat runs
on an otherwise idle host before drawing conclusions. Access logging is disabled
for Python, and neither framework performs request-time auth database lookups on
these public task reads. Writes are deliberately excluded: comparing SQLite lock
contention requires resetting the database between frameworks.
