import argparse
import sqlite3
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BENCHMARK_DIR = Path(__file__).resolve().parent
MIGRATIONS = ROOT / "examples/cli_fullstack/migrations"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rows", type=int, default=10_000)
    parser.add_argument("--database", type=Path, default=BENCHMARK_DIR / "benchmark.sqlite")
    args = parser.parse_args()
    database = args.database.resolve()
    if database.exists():
        database.unlink()
    subprocess.run(
        [
            "atlas",
            "migrate",
            "apply",
            "--dir",
            f"file://{MIGRATIONS}",
            "--url",
            f"sqlite://{database}?mode=rwc",
        ],
        check=True,
    )
    with sqlite3.connect(database) as connection:
        connection.execute(
            "INSERT INTO auth_users (id, username, password_hash, is_active, is_staff, is_admin, is_superuser) VALUES (1, 'benchmark', 'unused', 1, 0, 0, 0)"
        )
        connection.executemany(
            "INSERT INTO tasks_task (id, author_id, name, created_at, updated_at) VALUES (?, 1, ?, ?, ?)",
            (
                (
                    row_id,
                    f"benchmark task {row_id:05d}",
                    (datetime(2026, 1, 1, tzinfo=timezone.utc) + timedelta(seconds=row_id)).isoformat().replace("+00:00", "Z"),
                    (datetime(2026, 1, 1, tzinfo=timezone.utc) + timedelta(seconds=row_id)).isoformat().replace("+00:00", "Z"),
                )
                for row_id in range(1, args.rows + 1)
            ),
        )
        connection.commit()
    print(f"prepared {args.rows} tasks in {database}")


if __name__ == "__main__":
    main()
