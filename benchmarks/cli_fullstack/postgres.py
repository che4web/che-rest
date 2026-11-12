"""Disposable PostgreSQL HTTP comparison; never connects to an application database."""

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import socket
import statistics
import subprocess
import sys

import psycopg2
from psycopg2.extras import execute_values

import run as runner


PATHS = {
    "list": "/api/tasks/?limit=50&ordering=-created_at",
    "filtered-list": "/api/tasks/?limit=50&name__contains=task",
    "retrieve": "/api/tasks/500/",
}


def command(args, log=None):
    result = subprocess.run(list(map(str, args)), text=True, capture_output=True)
    if log:
        log.write_text(result.stdout + result.stderr)
    result.check_returncode()
    return result.stdout.strip()


def measure(url, requests, concurrency, raw):
    text = command(["ab", "-k", "-n", requests, "-c", concurrency, url], raw)
    values = runner.parse_ab(text)
    if values["failed_requests"] or values["non_2xx_responses"]:
        raise RuntimeError(f"invalid measurement: {values}; see {raw}")
    return values


def seed(url, binary, rows, output):
    command([binary, url], output / "schema.log")
    with psycopg2.connect(url) as db, db.cursor() as cursor:
        cursor.execute("INSERT INTO auth_users (id,username,password_hash,is_active,is_staff,is_admin,is_superuser) "
                       "VALUES (1,'benchmark','unused',true,false,false,false)")
        start = datetime(2026, 1, 1, tzinfo=timezone.utc)
        execute_values(cursor, "INSERT INTO tasks_task "
                       "(id,author_id,assignee_id,name,status,created_at,updated_at) VALUES %s", [
                           (i, 1, 1 if i % 2 == 0 else None, f"benchmark task {i:05d}",
                            ("draft", "in_progress", "done")[(i - 1) % 3],
                            start + timedelta(seconds=i), start + timedelta(seconds=i))
                           for i in range(1, rows + 1)
                       ])
        cursor.execute("ANALYZE")


def check_parity(url, binary, workers, rust_pool_size=4, django_pool_size=0):
    expected = {}
    paths = {**PATHS, "page": "/api/tasks/?limit=7&offset=3&ordering=id",
             "empty": "/api/tasks/?limit=50&name__contains=missing-benchmark-task"}
    for framework, port in runner.SERVERS:
        with runner.server(framework, port, Path("unused"), workers, postgres_url=url, rust_binary=binary,
                           rust_pool_size=rust_pool_size, django_pool_size=django_pool_size):
            for name, path in paths.items():
                payload = runner.request(f"http://127.0.0.1:{port}{path}")
                runner.validate_payload(payload, name)
                if framework == "che-rest":
                    expected[name] = payload
                elif payload != expected[name]:
                    raise AssertionError(f"JSON mismatch: {framework}, {path}\n"
                                         f"Rust: {str(expected[name])[:1600]}\nPython: {str(payload)[:1600]}")
    print("PostgreSQL JSON parity: all four servers, five paths", flush=True)


def save_plans(db, output, indexed):
    base = ("SELECT t.id,t.author_id,u.username,t.assignee_id,t.name,t.status,t.created_at,t.updated_at "
            "FROM tasks_task t JOIN auth_users u ON u.id=t.author_id")
    queries = {
        "list": base + " ORDER BY t.created_at DESC LIMIT 50",
        "filtered-list": base + " WHERE t.name LIKE '%task%' ORDER BY t.id LIMIT 50",
        "retrieve": base + " WHERE t.id=500",
        "count-direct": "SELECT count(*) FROM tasks_task",
        "count-join": "SELECT count(*) FROM tasks_task t JOIN auth_users u ON u.id=t.author_id",
    }
    with db.cursor() as cursor:
        plans = {}
        for name, query in queries.items():
            cursor.execute("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) " + query)
            plans[name] = cursor.fetchone()[0]
    (output / f"plans-{'indexed' if indexed else 'unindexed'}.json").write_text(json.dumps(plans, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path, help="new directory (must not exist)")
    parser.add_argument("--rust-bin-dir", required=True, type=Path, help="PostgreSQL release binaries")
    parser.add_argument("--rows", type=runner.positive, default=10_000)
    parser.add_argument("--requests", type=runner.positive, default=10_000)
    parser.add_argument("--concurrency", type=runner.positive, default=50)
    parser.add_argument("--workers", type=runner.positive, default=8)
    parser.add_argument("--rust-pool-size", type=runner.positive, default=4)
    parser.add_argument("--django-pool-size", type=runner.positive, default=0,
                        help="optional psycopg3 pool size per Django worker (default: disabled)")
    parser.add_argument("--warmup", type=runner.positive, default=500)
    parser.add_argument("--repetitions", type=runner.positive, default=3)
    args = parser.parse_args()
    if args.rows < 500 or args.concurrency > args.requests:
        parser.error("requires >=500 rows and concurrency <= requests")
    binary = args.rust_bin_dir.resolve() / "cli_fullstack"
    schema_binary = args.rust_bin_dir.resolve() / "benchmark_schema"
    if not binary.is_file() or not schema_binary.is_file():
        parser.error("build PostgreSQL release binaries first")
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    data = output / "data"
    command(["initdb", "-D", data, "-A", "trust", "--encoding=UTF8", "--no-locale"], output / "initdb.log")
    command(["pg_ctl", "-D", data, "-l", output / "postgres.log", "-o",
             f"-p {port} -k {output} -h 127.0.0.1 -c timezone=UTC", "-w", "start"], output / "start.log")
    url = f"postgresql://{os.environ['USER']}@127.0.0.1:{port}/postgres?sslmode=disable"
    try:
        seed(url, schema_binary, args.rows, output)
        with psycopg2.connect(url) as db:
            db.autocommit = True
            with db.cursor() as cursor:
                cursor.execute("SELECT version()")
                pg_version = cursor.fetchone()[0]
                settings = {}
                for setting in ["shared_buffers", "work_mem", "max_connections", "fsync", "TimeZone"]:
                    cursor.execute("SELECT current_setting(%s)", (setting,))
                    settings[setting] = cursor.fetchone()[0]
            metadata = {
                "backend": "postgres", "http": True, "rows": args.rows,
                "requests_per_measurement": args.requests, "concurrency": args.concurrency,
                "python_workers": args.workers, "rust_pool_size": args.rust_pool_size,
                "django_pool_size_per_worker": args.django_pool_size,
                "django_pool_size_total": args.django_pool_size * args.workers,
                "django_driver": json.loads(command([
                    "uv", "run", "--exact", "--locked", "--project", runner.BENCHMARK / "django",
                    *(["--extra", "pool"] if args.django_pool_size else []),
                    "python", "-c", "import json; " + (
                        "import psycopg, psycopg_pool; print(json.dumps({'driver': 'psycopg3', 'version': psycopg.__version__, 'pool_version': psycopg_pool.__version__}))"
                        if args.django_pool_size else
                        "import psycopg2; print(json.dumps({'driver': 'psycopg2', 'version': psycopg2.__version__}))"
                    ),
                ])),
                "warmup_requests": max(args.warmup, args.concurrency), "repetitions": args.repetitions,
                "postgres": pg_version, "postgres_config": settings,
                "cpu_count": os.cpu_count(), "platform": platform.platform(),
                "cpu_model": next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                                   if line.startswith("model name")), "unknown"),
                "load_average_before": os.getloadavg(),
                "tokio_worker_threads_env": os.environ.get("TOKIO_WORKER_THREADS"),
                "python": sys.version, "psycopg2": psycopg2.__version__,
                "rustc": command(["rustc", "--version"]),
                "rest_commit": command(["git", "-C", runner.ROOT, "rev-parse", "HEAD"]),
                "orm_commit": command(["git", "-C", runner.ROOT / "../che-orm", "rev-parse", "HEAD"]),
                "source_sha256": {str(path.relative_to(runner.ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                                  for path in [Path(__file__), Path(runner.__file__),
                                               runner.BENCHMARK / "fastapi/app.py",
                                               runner.BENCHMARK / "fastapi_sqlalchemy/app.py",
                                               runner.BENCHMARK / "django/config/settings.py",
                                               runner.BENCHMARK / "django/tasks/views.py",
                                               runner.BENCHMARK / "django/tasks/models.py",
                                               *runner.BENCHMARK.glob("*/uv.lock")]},
                "rust_binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "connection_models": {
                    "che-rest": f"one process, pool {args.rust_pool_size}, Tokio multithread defaults",
                    "fastapi": "synchronous endpoints, new libpq connection per request",
                    "fastapi-sqlalchemy": "synchronous endpoints, pool 4 per worker, max_overflow 0",
                    "django": (f"ASGI, CONN_MAX_AGE 0, psycopg3 pool {args.django_pool_size} per worker"
                               if args.django_pool_size else "ASGI, CONN_MAX_AGE 0, new connection per request"),
                },
            }
            results = []
            report = {"metadata": metadata, "results": results}
            for indexed in [False, True]:
                if indexed:
                    with db.cursor() as cursor:
                        cursor.execute("CREATE INDEX benchmark_tasks_created_at ON tasks_task(created_at)")
                        cursor.execute("ANALYZE tasks_task")
                check_parity(url, binary, args.workers, args.rust_pool_size, args.django_pool_size)
                save_plans(db, output, indexed)
                for repetition in range(args.repetitions):
                    # Rotate server order to reduce systematic order bias.
                    servers = runner.SERVERS[repetition:] + runner.SERVERS[:repetition]
                    for framework, port in servers:
                        with runner.server(framework, port, Path("unused"), args.workers,
                                           postgres_url=url, rust_binary=binary, rust_pool_size=args.rust_pool_size,
                                           django_pool_size=args.django_pool_size):
                            for scenario, path in PATHS.items():
                                label = f"{'indexed' if indexed else 'unindexed'}-{repetition}-{framework}-{scenario}"
                                endpoint = f"http://127.0.0.1:{port}{path}"
                                measure(endpoint, max(args.warmup, args.concurrency), args.concurrency,
                                        output / f"{label}-warmup.txt")
                                values = {"framework": framework, "scenario": scenario,
                                          "indexed": indexed, "repetition": repetition,
                                          **measure(endpoint, args.requests, args.concurrency, output / f"{label}.txt")}
                                results.append(values)
                                (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
                                print(json.dumps(values), flush=True)
            summary = []
            for indexed in [False, True]:
                for framework, _ in runner.SERVERS:
                    for scenario in PATHS:
                        runs = [item for item in results if item["indexed"] == indexed
                                and item["framework"] == framework and item["scenario"] == scenario]
                        summary.append({"indexed": indexed, "framework": framework, "scenario": scenario,
                                        **{key: statistics.median(item[key] for item in runs)
                                           for key in ["requests_per_second", "mean_latency_ms", "p95_latency_ms"]}})
            (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
            metadata["load_average_after"] = os.getloadavg()
            (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    finally:
        command(["pg_ctl", "-D", data, "-m", "fast", "-w", "stop"], output / "stop.log")


if __name__ == "__main__":
    main()
