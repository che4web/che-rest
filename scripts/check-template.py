#!/usr/bin/env python3
"""Verify cargo-generate and CLI REST scaffolds, migrations and real HTTP CRUD."""

import argparse
import hashlib
from itertools import product
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def run(command, *, cwd, env):
    print("$ " + " ".join(map(str, command)), flush=True)
    return subprocess.run(command, cwd=cwd, env=env, check=True)


def fails(command, *, cwd, env, message):
    result = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True)
    assert result.returncode != 0, result.stdout + result.stderr
    assert message in result.stdout + result.stderr, result.stdout + result.stderr


def verify_http(project, binary, env):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    config = project / "app.toml"
    config.write_text(config.read_text().replace("port = 3000", f"port = {port}"))
    base = f"http://127.0.0.1:{port}"

    def request(path, method="GET", data=None):
        body = None if data is None else json.dumps(data).encode()
        req = urllib.request.Request(base + path, data=body, method=method,
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as response:
            value = response.read()
            return json.loads(value) if value else None

    with tempfile.TemporaryFile() as output:
        server = subprocess.Popen([str(binary)], cwd=project, env=env,
                                  stdout=output, stderr=output)
        try:
            for _ in range(100):
                if server.poll() is not None:
                    output.seek(0)
                    raise AssertionError(output.read().decode())
                try:
                    request("/api/openapi.json")
                    break
                except (urllib.error.URLError, TimeoutError):
                    time.sleep(0.1)
            else:
                raise AssertionError("generated HTTP server did not start")
            created = request("/api/task/", "POST", {"name": "Template HTTP test"})
            key = created["id"]
            assert request(f"/api/task/{key}/")["name"] == "Template HTTP test"
            updated = request(f"/api/task/{key}/", "PATCH", {"name": "Updated"})
            assert updated["name"] == "Updated"
            request(f"/api/task/{key}/", "DELETE")
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--layout", choices=["single", "modular"])
    parser.add_argument("--generator", choices=["cargo-generate", "cli"])
    parser.add_argument("--published", action="store_true",
                        help="use crates.io dependencies after publication")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    orm = (root / "../che-orm").resolve()
    env = dict(os.environ)
    target = Path(env.get("CARGO_TARGET_DIR", root / "target/template-check")).resolve()
    env["CARGO_TARGET_DIR"] = str(target / ("published" if args.published else "local"))
    layouts = [args.layout] if args.layout else ["single", "modular"]
    generators = [args.generator] if args.generator else ["cargo-generate", "cli"]
    with tempfile.TemporaryDirectory(prefix="che-rest-template-") as directory:
        destination = Path(directory)
        # Generation must not attach the project to an enclosing workspace.
        parent = destination / "Cargo.toml"
        parent.write_text("[workspace]\nmembers = []\n")
        for generator, layout, auth in product(generators, layouts, [False, True]):
            name = f"rest-{generator}-{layout}-{'auth' if auth else 'public'}"
            project = destination / name
            paths = [] if args.published else [
                "--che-rest-path", str(root), "--che-orm-path", str(orm),
            ]
            if generator == "cargo-generate":
                run([
                    "cargo", "generate", "--path", str(root / "templates/project"),
                    "--name", name, "--destination", str(destination),
                    "--define", f"layout={layout}", "--define", f"with_auth={str(auth).lower()}",
                    "--define", f"che_rest_path={'' if args.published else root}",
                    "--define", f"che_orm_path={'' if args.published else orm}",
                    "--silent", "--vcs", "none", "--no-workspace",
                ], cwd=root, env=env)
            else:
                run([
                    "cargo", "run", "--bin", "che-rest", "--", "startproject", name,
                    "--out", str(destination), "--layout", layout,
                    *paths, *(["--with-auth"] if auth else []),
                ], cwd=root, env=env)
            assert parent.read_text() == "[workspace]\nmembers = []\n"
            source = project / ("src" if layout == "single" else "src/apps/tasks")
            assert (project / "src/models.rs").exists() == (layout == "single")
            assert (project / "src/apps/tasks").exists() == (layout == "modular")
            assert '#[orm(app = super::APP_NAME)]' in (source / "models.rs").read_text()
            history = source / "migrations"
            assert [p.name for p in history.iterdir()] == ["mod.rs"]
            registry = (project / "src/apps/mod.rs").read_text()
            assert ("che_rest::auth::module()" in registry) == auth

            def cargo(*options):
                return ["cargo", *options]

            def manage(*options):
                return cargo("run", "--bin", "manage", "--", *options)

            # CLI-generated source is formatted here, while the cargo-generate
            # skeleton must already be formatted before its first command.
            if generator == "cli":
                run(cargo("fmt"), cwd=project, env=env)
            run(cargo("fmt", "--check"), cwd=project, env=env)
            fails(manage("makemigrations", "--check"), cwd=project, env=env,
                  message="model changes")
            run(manage("makemigrations", "--name", "initial"), cwd=project, env=env)
            initial = history / "m0001_initial.rs"
            assert initial.exists() and "tasks_task" in initial.read_text()
            digest = hashlib.sha256(initial.read_bytes()).digest()
            for command in [manage("migrate"), manage("migrate"), manage("migrate", "status"),
                            manage("makemigrations", "--check")]:
                run(command, cwd=project, env=env)

            for invalid in ["mod", "../escape", "installed_apps"]:
                fails(manage("createapp", invalid), cwd=project, env=env, message="app name")
            fails(manage("createapp", "tasks"), cwd=project, env=env, message="already installed")
            run(manage("createapp", "blog", "--model", "Post"), cwd=project, env=env)
            blog = project / "src/apps/blog"
            assert 'pub const APP_NAME: &str = "blog";' in (blog / "mod.rs").read_text()
            models = blog / "models.rs"
            assert '#[orm(app = super::APP_NAME)]' in models.read_text()
            fails(manage("createapp", "blog"), cwd=project, env=env, message="already installed")
            task = "crate::models::Task" if layout == "single" else "crate::apps::tasks::models::Task"
            models.write_text(f"use {task};\n" + models.read_text().replace(
                "    pub name: String,",
                '    pub name: String,\n    #[orm(foreign_key = Task, on_delete = "cascade")]\n'
                '    pub task_id: Option<i64>,',
            ))
            run(manage("makemigrations", "--name", "add_blog"), cwd=project, env=env)
            assert hashlib.sha256(initial.read_bytes()).digest() == digest
            run(manage("migrate"), cwd=project, env=env)
            with sqlite3.connect(project / "db.sqlite") as db:
                db.execute("PRAGMA foreign_keys = ON")
                key = db.execute("INSERT INTO tasks_task (name) VALUES ('parent') RETURNING id").fetchone()[0]
                db.execute("INSERT INTO blog_post (name, task_id) VALUES ('child', ?)", (key,))
                assert db.execute("SELECT task_id FROM blog_post").fetchone() == (key,)
            run(cargo("fmt"), cwd=project, env=env)
            for command in [cargo("fmt", "--check"), cargo("test"),
                            cargo("clippy", "--all-targets", "--", "-D", "warnings"),
                            manage("makemigrations", "--check"), cargo("build", "--bin", name)]:
                run(command, cwd=project, env=env)
            verify_http(project, Path(env["CARGO_TARGET_DIR"]) / "debug" / name, env)
            print(f"Verified {generator}: layout={layout}, auth={auth}", flush=True)


if __name__ == "__main__":
    main()
