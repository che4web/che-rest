# cli_fullstack read benchmark

Compares the current `cli_fullstack` task list/retrieve endpoints with FastAPI using
direct `sqlite3`, FastAPI using SQLAlchemy ORM, and Django ORM. Every server reads
the same SQLite database and returns the same `TaskSerializer` JSON, including
nested `author`, nullable `assignee_id`, `status`, and timestamps. ORM servers load
the author eagerly, as the REST ViewSet does.

Requirements: Rust/Cargo, Python 3.11+, `uv`, and Apache Bench (`ab`). Atlas is not
used. Run commands from the repository root.

For the separate PostgreSQL HTTP matrix, see [PostgreSQL comparison](#postgresql-http-comparison).

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

The seed writes the ORM's canonical SQLite datetime format: UTC TEXT with
exactly nine fractional digits. Older fixture databases must be recreated or
explicitly migrated before benchmarking this ORM version; mixed datetime
formats no longer receive a parsing collation at query time. No datetime index
is added by the seed, so the list scenario still measures unindexed sorting.

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
python3 benchmarks/cli_fullstack/run.py --requests 10000 --concurrency 50 --workers 8
```

The read-only scenarios are:

- `GET /api/tasks/?limit=50&ordering=-created_at`
- `GET /api/tasks/?limit=50&name__contains=task`
- `GET /api/tasks/500/`

Servers run **one at a time** on loopback ports 3101–3104. Each scenario has a
warmup (200 requests by default, configurable with `--warmup`) followed by Apache
Bench with HTTP keep-alive. Python uses eight Uvicorn worker processes by default;
Rust uses its Tokio multithread runtime and a four-connection database pool.
These are different concurrency models, not an equal-CPU allocation.

Worker count is also **not** a Python connection limit: FastAPI's synchronous
endpoints run in a threadpool and open a connection per request. A worker can
therefore execute more than one SQL request concurrently, while Rust is bounded
by its pool. JSON parity does not make the SQL identical either: the ORM loads
the complete author model and its count retains a JOIN, whereas the direct
`sqlite3` implementation projects only the needed author fields and counts tasks
without that JOIN.

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

## Interpretation and known SQLite limitation

Read [SQLite performance investigation](SQLITE-PERFORMANCE.md) before drawing
conclusions from the sorted-list result. It records the comparison with and
without a common datetime index, separate SQL measurements, pool/process controls,
and isolated SQLite build/initialization experiments. Increasing the Rust pool
from four to eight connections made the unindexed workload slower in the default
bundled build; this was reproduced without HTTP or ORM code.

The initial explanation attributing this HTTP endpoint's slowdown to datetime
parsing inside `che_datetime` was incorrect: the REST ordering adapter rebuilt
the field with `ModelField::new`, so that older ORDER BY did not request the
collation. A direct ORM collation microbenchmark is not an HTTP before/after
measurement. Canonical UTC TEXT fixes the storage/comparison contract and enables
ordinary indexes, but must not receive credit for unrelated SQL concurrency or
build-configuration differences.

The investigated `libsqlite3-sys` bundled build enables
`SQLITE_ENABLE_MEMORY_MANAGEMENT`, which selects a shared page-cache management
group with a common mutex. Disabling that option improved concurrent throughput
in diagnostic builds; memory allocation statistics became another bottleneck
afterward. **These experimental settings have not been adopted as framework
defaults or added as supported runner options.** They have memory-management and
initialization implications and are not equivalent to disabling thread safety.

Keep indexed and unindexed fixtures/results separate. To compare indexing, copy
a disposable seed, add the same ordinary `created_at` index to that copy, and run
all four servers against it with distinct `--database` and `--output` paths. The
standard runner still fixes the Rust pool at four connections; `--workers` changes
only Python workers. Pool-size and initialization A/B tests require a separate
diagnostic harness, not an undocumented `run.py` option. The checked-in
`results.json` is a historical run, not the later diagnostic data or an aggregate
across configurations.

## PostgreSQL HTTP comparison

Saved report: [2026-11-12 PostgreSQL HTTP comparison](postgres/2026-11-12/README.md).
Follow-up: [pool 32 versus pool 4](postgres/2026-11-12-pool32/README.md).
Pooled Django: [psycopg 3, max 32 connections](postgres/2026-11-12-django-pool32/README.md).

`postgres.py` runs the same four HTTP implementations against one disposable
local PostgreSQL cluster. It requires `initdb`, `pg_ctl`, `psql`/libpq tooling,
Apache Bench, `uv` and Rust; do not run it as root. The Python projects pin
`psycopg2-binary` in their lock files. No application database is used.

```bash
CARGO_TARGET_DIR=/tmp/opencode/che-rest-http-pg-target \
  cargo build --release --manifest-path examples/cli_fullstack/Cargo.toml \
  --no-default-features --features postgres --bins
uv run --project benchmarks/cli_fullstack/fastapi \
  python benchmarks/cli_fullstack/postgres.py \
  --rust-bin-dir /tmp/opencode/che-rest-http-pg-target/release \
  --output-dir /tmp/opencode/NEW-postgres-http-results
```

The output directory must be new. The script initializes its own cluster there,
applies compiled fixture migrations, seeds 10,000 rows and runs ANALYZE. It
checks exact parsed JSON parity on five paths before each index variant, then
runs list/filter/detail sequentially with 500 warmup requests, 10,000 measured
requests, concurrency 50, eight Python workers and three repetitions. Server
order rotates between repetitions. First there is no datetime index; then the
same database receives an ordinary `created_at` index. Both variants remain
warm-cache tests, not disk-I/O benchmarks. Writes are not measured.

Every Apache Bench output (including warmup), raw measurements, median summaries,
representative SQL execution plans, source/binary hashes and PostgreSQL settings
are saved. Any failed request, non-2xx response or JSON mismatch rejects the run.
Servers and the cluster are stopped on exit; the disposable cluster files remain
in the output directory for investigation. Only JSON/text reports, not database
files or local connection URLs, belong in saved repository results.

Connection/concurrency models intentionally follow the existing applications:

- che-rest: one Tokio multithreaded process and a four-connection pool.
- FastAPI/direct psycopg2: synchronous threadpool endpoint, a new connection per
  request, closed explicitly after reading.
- FastAPI/SQLAlchemy: synchronous endpoints, a four-connection pool **per worker**
  (up to 32 pooled connections with eight workers), no overflow.
- Django/ASGI: `CONN_MAX_AGE=0`, a new connection per request.

Thus this is not a controlled equal-pool, equal-core or ORM-only comparison.
Connection setup, eager-author projections, count-query JOINs, driver/runtime
behavior and JSON serialization are included. Keep these differences visible
when interpreting throughput or comparing it with SQLite results.

Use `postgres.py --rust-pool-size 32` with a new output directory to repeat the
matrix with 32 che-rest connections (instead of the default four). This changes
only the benchmark's temporary server configuration, not application defaults or
the Python connection models. The chosen limit is recorded in result metadata.

### Optional Django PostgreSQL pool

Add `--django-pool-size 4 --workers 8` to configure a fixed four-connection pool
per Django worker (32 connections maximum across the eight processes). Combine
this with `--rust-pool-size 32` to align the configured maximum connection budget
of che-rest, SQLAlchemy and Django. Direct FastAPI remains unpooled.

```bash
uv run --project benchmarks/cli_fullstack/fastapi \
  python benchmarks/cli_fullstack/postgres.py \
  --rust-bin-dir /tmp/opencode/che-rest-http-pg-target/release \
  --rust-pool-size 32 --django-pool-size 4 --workers 8 \
  --output-dir /tmp/opencode/NEW-django-pooled-results
```

This selects the Django project's optional `pool` dependency extra and Django's
native PostgreSQL pooling through psycopg 3 / psycopg_pool. `CONN_MAX_AGE` stays
zero: request cleanup returns connections to the shared per-process pool rather
than preserving a connection per request thread. Pools use `min_size=max_size`
equal to the configured size and a 30-second acquisition timeout. The runner uses
`uv run --exact --locked`, so a later unpooled run removes the psycopg 3 extra
and again selects the original psycopg2 driver rather than silently retaining it.

Pool sizes and Django driver/pool versions are recorded in report metadata.
Compared with the old unpooled series, this changes **both pooling and the Django
driver** (psycopg2 -> psycopg 3), so any performance change cannot be attributed
exclusively to pooling. Other implementations, requests and database settings
remain unchanged. Use a fresh output directory and preserve the earlier reports.
