import argparse
import json
import os
import signal
import subprocess
import time
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BENCHMARK = Path(__file__).resolve().parent
RUST_BINARY = ROOT / "examples/cli_fullstack/target/release/cli_fullstack"


def wait_for(url: str, process: subprocess.Popen) -> None:
    for _ in range(300):
        if process.poll() is not None:
            raise RuntimeError(f"server exited with status {process.returncode}")
        try:
            urllib.request.urlopen(url, timeout=0.2)
            return
        except Exception:
            time.sleep(0.1)
    raise TimeoutError(f"server did not become ready: {url}")


def start_rust() -> subprocess.Popen:
    return subprocess.Popen([str(RUST_BINARY)], cwd=BENCHMARK, start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def start_fastapi(workers: int) -> subprocess.Popen:
    environment = {**os.environ, "BENCHMARK_DATABASE": str(BENCHMARK / "benchmark.sqlite")}
    return subprocess.Popen(
        ["uv", "run", "--project", str(BENCHMARK / "fastapi"), "uvicorn", "app:app", "--host", "127.0.0.1", "--port", "3102", "--workers", str(workers)],
        cwd=BENCHMARK / "fastapi",
        env=environment,
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def start_fastapi_sqlalchemy(workers: int) -> subprocess.Popen:
    environment = {**os.environ, "BENCHMARK_DATABASE": str(BENCHMARK / "benchmark.sqlite")}
    return subprocess.Popen(
        ["uv", "run", "--project", str(BENCHMARK / "fastapi_sqlalchemy"), "uvicorn", "app:app", "--host", "127.0.0.1", "--port", "3103", "--workers", str(workers)],
        cwd=BENCHMARK / "fastapi_sqlalchemy",
        env=environment,
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def start_django(workers: int) -> subprocess.Popen:
    environment = {**os.environ, "BENCHMARK_DATABASE": str(BENCHMARK / "benchmark.sqlite")}
    return subprocess.Popen(
        ["uv", "run", "--project", str(BENCHMARK / "django"), "uvicorn", "config.asgi:application", "--host", "127.0.0.1", "--port", "3104", "--workers", str(workers)],
        cwd=BENCHMARK / "django",
        env=environment,
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def stop(process: subprocess.Popen) -> None:
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            try:
                process.kill()
            except ProcessLookupError:
                pass


def request(url: str) -> bytes:
    with urllib.request.urlopen(url) as response:
        return response.read()


def parity(retrieve_id: int) -> None:
    rust = start_rust()
    fastapi = start_fastapi(4)
    sqlalchemy = start_fastapi_sqlalchemy(4)
    django = start_django(4)
    try:
        wait_for("http://127.0.0.1:3101/api/tasks/?limit=50&ordering=-created_at", rust)
        wait_for("http://127.0.0.1:3102/api/tasks/?limit=50&ordering=-created_at", fastapi)
        wait_for("http://127.0.0.1:3103/api/tasks/?limit=50&ordering=-created_at", sqlalchemy)
        wait_for("http://127.0.0.1:3104/api/tasks/?limit=50&ordering=-created_at", django)
        paths = ["/api/tasks/?limit=50&ordering=-created_at", "/api/tasks/?limit=50&name__contains=task", f"/api/tasks/{retrieve_id}/"]
        for path in paths:
            rust_json = json.loads(request(f"http://127.0.0.1:3101{path}"))
            fastapi_json = json.loads(request(f"http://127.0.0.1:3102{path}"))
            sqlalchemy_json = json.loads(request(f"http://127.0.0.1:3103{path}"))
            django_json = json.loads(request(f"http://127.0.0.1:3104{path}"))
            if rust_json != fastapi_json or rust_json != sqlalchemy_json or rust_json != django_json:
                raise AssertionError(f"response mismatch for {path}")
        print("parity: ok")
    finally:
        stop(rust)
        stop(fastapi)
        stop(sqlalchemy)
        stop(django)


def benchmark(name: str, url: str, requests: int, concurrency: int) -> dict:
    output = subprocess.run(
        ["ab", "-k", "-n", str(requests), "-c", str(concurrency), url],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    values = {}
    for line in output.splitlines():
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        if key in {"Requests per second", "Time per request", "Failed requests"}:
            values[key] = value.strip()
    return {"scenario": name, **values}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--requests", type=int, default=10_000)
    parser.add_argument("--concurrency", type=int, default=50)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--retrieve-id", type=int, default=500)
    parser.add_argument("--parity", action="store_true")
    args = parser.parse_args()
    if args.parity:
        parity(args.retrieve_id)
        return
    rust = start_rust()
    fastapi = start_fastapi(args.workers)
    sqlalchemy = start_fastapi_sqlalchemy(args.workers)
    django = start_django(args.workers)
    try:
        wait_for("http://127.0.0.1:3101/api/tasks/?limit=50", rust)
        wait_for("http://127.0.0.1:3102/api/tasks/?limit=50", fastapi)
        wait_for("http://127.0.0.1:3103/api/tasks/?limit=50", sqlalchemy)
        wait_for("http://127.0.0.1:3104/api/tasks/?limit=50", django)
        scenarios = {
            "list": "/api/tasks/?limit=50&ordering=-created_at",
            "filtered-list": "/api/tasks/?limit=50&name__contains=task",
            "retrieve": f"/api/tasks/{args.retrieve_id}/",
        }
        results = []
        for framework, port in (("che-rest", 3101), ("fastapi", 3102), ("fastapi-sqlalchemy", 3103), ("django", 3104)):
            for name, path in scenarios.items():
                results.append({"framework": framework, **benchmark(name, f"http://127.0.0.1:{port}{path}", args.requests, args.concurrency)})
        print(json.dumps(results, indent=2))
    finally:
        stop(rust)
        stop(fastapi)
        stop(sqlalchemy)
        stop(django)


if __name__ == "__main__":
    main()
