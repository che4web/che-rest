# cli_fullstack Read Benchmark

This compares the `cli_fullstack` task list/retrieve endpoints with equivalent Python servers:
FastAPI using direct `sqlite3`, FastAPI using SQLAlchemy ORM, and Django ORM. All servers read the
same SQLite database and return the same nested task JSON.

## Prepare

From the repository root:

```bash
cargo build --release --manifest-path examples/cli_fullstack/Cargo.toml
python3 benchmarks/cli_fullstack/prepare.py --rows 10000
```

Preparation removes and recreates `benchmark.sqlite`, applies the checked-in Atlas migration, and
seeds one user plus 10,000 tasks. It never uses `db.sqlite`.

## Validate

```bash
python3 benchmarks/cli_fullstack/run.py --parity
```

The parity scenarios are:

- `GET /api/tasks/?limit=50&ordering=-created_at`
- `GET /api/tasks/?limit=50&name__contains=task`
- `GET /api/tasks/500/`

## Benchmark

```bash
python3 benchmarks/cli_fullstack/run.py --requests 10000 --concurrency 50 --workers 4
```

The benchmark runs Rust and FastAPI sequentially against the same database file. It reports Apache
Bench requests/sec, mean time/request, and failed requests for list, filtered-list, and retrieve.
The default workload is read-only by design: writes would measure SQLite locking and require a DB
reset between frameworks rather than provide a clean framework comparison.

Use `--retrieve-id` when the seed contains fewer than 500 rows, for example `--retrieve-id 50`.
The `--parity` flag only validates responses and does not run the load benchmark.

Requirements: Rust/Cargo, Python 3.11+, `uv`, and Apache Bench (`ab`). Atlas is only required when
regenerating migrations.

The Django server can also be started directly:

```bash
BENCHMARK_DATABASE="$PWD/benchmarks/cli_fullstack/benchmark.sqlite" \
uv run --project benchmarks/cli_fullstack/django \
  uvicorn config.asgi:application --host 127.0.0.1 --port 3104 --workers 4
```

The SQLAlchemy server can also be started directly:

```bash
BENCHMARK_DATABASE="$PWD/benchmarks/cli_fullstack/benchmark.sqlite" \
uv run --project benchmarks/cli_fullstack/fastapi_sqlalchemy \
  uvicorn app:app --host 127.0.0.1 --port 3103 --workers 4
```
