import argparse
import json
import os
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from contextlib import closing, contextmanager
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BENCHMARK = Path(__file__).resolve().parent
RUST_BINARY = ROOT / "examples/cli_fullstack/target/release/cli_fullstack"
SERVERS = (("che-rest", 3101), ("fastapi", 3102), ("fastapi-sqlalchemy", 3103), ("django", 3104))


def request(url: str):
    with urllib.request.urlopen(url, timeout=10) as response:
        return json.load(response)


def wait_for(url: str, process: subprocess.Popen) -> None:
    deadline = time.monotonic() + 30
    last_error = "no response"
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited with status {process.returncode}")
        try:
            request(url)
            return
        except urllib.error.HTTPError as error:
            # A live but broken route is not a server startup delay.
            raise RuntimeError(f"{url}: HTTP {error.code}: {error.read().decode(errors='replace')}") from error
        except (urllib.error.URLError, TimeoutError) as error:
            last_error = str(error)
            time.sleep(0.1)
    raise TimeoutError(f"server did not become ready: {url}: {last_error}")


def validate_database(database: Path, retrieve_id: int) -> int:
    with closing(sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)) as connection:
        columns = {row[1] for row in connection.execute("PRAGMA table_info(tasks_task)")}
        expected = {"id", "author_id", "assignee_id", "name", "status", "created_at", "updated_at"}
        if not expected <= columns:
            raise RuntimeError(f"outdated benchmark database (missing {sorted(expected - columns)}); run prepare.py")
        count = connection.execute("SELECT COUNT(*) FROM tasks_task").fetchone()[0]
        if not connection.execute("SELECT 1 FROM tasks_task WHERE id = ?", (retrieve_id,)).fetchone():
            raise RuntimeError(f"task {retrieve_id} does not exist; choose --retrieve-id from the seed")
        return count


@contextmanager
def server(name: str, port: int, database: Path, workers: int):
    with socket.socket() as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        probe.bind(("127.0.0.1", port))
    with tempfile.TemporaryDirectory(prefix="che-rest-bench-") as directory:
        directory = Path(directory)
        if name == "che-rest":
            config = (
                f"[database]\nurl = {json.dumps(f'sqlite://{database}')}\nmax_connections = 4\n"
                f'[server]\nhost = "127.0.0.1"\nport = {port}\napi_prefix = "/api"\n'
            )
            (directory / "app.toml").write_text(config)
            command, cwd = [str(RUST_BINARY)], directory
        else:
            project = BENCHMARK / name.replace("-", "_")
            application = "config.asgi:application" if name == "django" else "app:app"
            command = ["uv", "run", "--project", str(project), "uvicorn", application,
                       "--host", "127.0.0.1", "--port", str(port), "--workers", str(workers),
                       "--no-access-log"]
            cwd = project
        with (directory / "server.log").open("w+") as log:
            process = subprocess.Popen(command, cwd=cwd,
                                       env={**os.environ, "BENCHMARK_DATABASE": str(database)},
                                       start_new_session=True, stdout=log, stderr=log)
            try:
                wait_for(f"http://127.0.0.1:{port}/api/tasks/?limit=1", process)
                yield
            except Exception:
                log.flush()
                log.seek(0)
                print(f"{name} server log:\n{log.read()[-8000:]}", file=sys.stderr)
                raise
            finally:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()


def validate_payload(payload, name: str) -> None:
    tasks = [payload] if name.startswith("retrieve") else payload["results"]
    for task in tasks:
        if set(task) != {"id", "author", "assignee_id", "name", "status", "created_at", "updated_at"}:
            raise AssertionError(f"unexpected TaskSerializer fields: {sorted(task)}")


def parity(database: Path, workers: int, paths: dict) -> None:
    expected = {}
    for framework, port in SERVERS:
        with server(framework, port, database, workers):
            for name, path in paths.items():
                payload = request(f"http://127.0.0.1:{port}{path}")
                validate_payload(payload, name)
                if framework == "che-rest":
                    expected[name] = payload
                elif payload != expected[name]:
                    raise AssertionError(f"response mismatch: {framework}, {path}")
    print("parity: ok (all four servers)", flush=True)


def parse_ab(output: str) -> dict:
    values = {"failed_requests": 0, "non_2xx_responses": 0}
    for line in output.splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            if key == "Requests per second":
                values["requests_per_second"] = float(value.split()[0])
            elif key == "Time per request" and "across all concurrent requests" not in value:
                values["mean_latency_ms"] = float(value.split()[0])
            elif key == "Failed requests":
                values["failed_requests"] = int(value.strip())
            elif key == "Non-2xx responses":
                values["non_2xx_responses"] = int(value.strip())
        elif line.strip().startswith("95%"):
            values["p95_latency_ms"] = int(line.split()[1])
    if not {"requests_per_second", "mean_latency_ms", "p95_latency_ms"} <= values.keys():
        raise RuntimeError(f"incomplete Apache Bench output:\n{output}")
    return values


def benchmark(url: str, requests: int, concurrency: int) -> dict:
    completed = subprocess.run(["ab", "-k", "-n", str(requests), "-c", str(concurrency), url],
                               capture_output=True, text=True)
    if completed.returncode:
        raise RuntimeError(f"Apache Bench failed:\n{completed.stdout}\n{completed.stderr}")
    values = parse_ab(completed.stdout)
    if values["failed_requests"] or values["non_2xx_responses"]:
        raise RuntimeError(f"invalid benchmark run: {values}\n{completed.stdout}")
    return values


def positive(value: str) -> int:
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--database", type=Path, default=BENCHMARK / "benchmark.sqlite")
    parser.add_argument("--requests", type=positive, default=10_000)
    parser.add_argument("--concurrency", type=positive, default=50)
    parser.add_argument("--workers", type=positive, default=4)
    parser.add_argument("--retrieve-id", type=positive, default=500)
    parser.add_argument("--warmup", type=positive, default=200)
    parser.add_argument("--output", type=Path, default=BENCHMARK / "results.json")
    parser.add_argument("--parity", action="store_true")
    args = parser.parse_args()
    if args.concurrency > args.requests:
        parser.error("--concurrency must not exceed --requests")
    database = args.database.resolve()
    rows = validate_database(database, args.retrieve_id)
    paths = {
        "list": "/api/tasks/?limit=50&ordering=-created_at",
        "filtered-list": "/api/tasks/?limit=50&name__contains=task",
        "retrieve": f"/api/tasks/{args.retrieve_id}/",
    }
    # Include null/non-null assignees and all three enum variants in parity checks.
    parity_paths = {**paths, "page": "/api/tasks/?limit=7&offset=3&ordering=id",
                    "empty": "/api/tasks/?limit=50&name__contains=missing-benchmark-task"}
    parity(database, args.workers, parity_paths)
    if args.parity:
        return
    results = []
    for framework, port in SERVERS:
        with server(framework, port, database, args.workers):
            for name, path in paths.items():
                url = f"http://127.0.0.1:{port}{path}"
                benchmark(url, max(args.warmup, args.concurrency), args.concurrency)
                values = {"framework": framework, "scenario": name,
                          **benchmark(url, args.requests, args.concurrency)}
                results.append(values)
                print(json.dumps(values), flush=True)
    report = {"rows": rows, "requests_per_scenario": args.requests,
              "concurrency": args.concurrency, "python_workers": args.workers,
              "warmup_requests": max(args.warmup, args.concurrency), "results": results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"results: {args.output}")


if __name__ == "__main__":
    main()
