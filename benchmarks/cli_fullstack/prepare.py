import argparse
import sqlite3
import subprocess
import tempfile
from contextlib import closing
from datetime import datetime, timedelta, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BENCHMARK_DIR = Path(__file__).resolve().parent
SCHEMA_BINARY = ROOT / "examples/cli_fullstack/target/release/benchmark_schema"


def seed(database: Path, rows: int) -> None:
    subprocess.run(
        [str(SCHEMA_BINARY), f"sqlite://{database}"],
        check=True,
    )
    with closing(sqlite3.connect(database)) as connection, connection:
        connection.execute("PRAGMA foreign_keys = ON")
        connection.execute(
            "INSERT INTO auth_users (id, username, password_hash, is_active, is_staff, is_admin, is_superuser) VALUES (1, 'benchmark', 'unused', 1, 0, 0, 0)"
        )
        connection.executemany(
            "INSERT INTO tasks_task (id, author_id, assignee_id, name, status, created_at, updated_at) VALUES (?, 1, ?, ?, ?, ?, ?)",
            (
                (
                    row_id,
                    1 if row_id % 2 == 0 else None,
                    f"benchmark task {row_id:05d}",
                    ("draft", "in_progress", "done")[(row_id - 1) % 3],
                    (datetime(2026, 1, 1, tzinfo=timezone.utc) + timedelta(seconds=row_id)).strftime("%Y-%m-%dT%H:%M:%S.000000000Z"),
                    (datetime(2026, 1, 1, tzinfo=timezone.utc) + timedelta(seconds=row_id)).strftime("%Y-%m-%dT%H:%M:%S.000000000Z"),
                )
                for row_id in range(1, rows + 1)
            ),
        )
        connection.commit()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rows", type=int, default=10_000)
    parser.add_argument("--database", type=Path, default=BENCHMARK_DIR / "benchmark.sqlite")
    parser.add_argument("--force", action="store_true", help="replace an existing benchmark database")
    args = parser.parse_args()
    if args.rows < 1:
        parser.error("--rows must be positive")
    database = args.database.resolve()
    if not SCHEMA_BINARY.is_file():
        parser.error("build first: cargo build --release --manifest-path examples/cli_fullstack/Cargo.toml --bins")
    if database.exists() and not args.force:
        parser.error(f"database exists: {database}; use --force to replace it")
    if any(Path(f"{database}{suffix}").exists() for suffix in ("-wal", "-shm", "-journal")):
        parser.error("database has SQLite sidecar files; stop its users before replacing it")
    database.parent.mkdir(parents=True, exist_ok=True)
    # Build beside the destination, then publish only a completely migrated seed.
    with tempfile.TemporaryDirectory(prefix=".benchmark-seed-", dir=database.parent) as directory:
        staging = Path(directory) / "benchmark.sqlite"
        seed(staging, args.rows)
        staging.replace(database)
    print(f"prepared {args.rows} tasks in {database}")


if __name__ == "__main__":
    main()
