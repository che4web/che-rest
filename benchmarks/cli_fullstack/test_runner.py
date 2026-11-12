import importlib.util
import json
import os
import runpy
import sqlite3
import subprocess
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import Mock, patch
from types import SimpleNamespace


SPEC = importlib.util.spec_from_file_location("benchmark_runner", Path(__file__).with_name("run.py"))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)
PREPARE_SPEC = importlib.util.spec_from_file_location("benchmark_prepare", Path(__file__).with_name("prepare.py"))
prepare = importlib.util.module_from_spec(PREPARE_SPEC)
PREPARE_SPEC.loader.exec_module(prepare)

AB_OUTPUT = """Failed requests:        0
Requests per second:    1234.56 [#/sec] (mean)
Time per request:       40.500 [ms] (mean)
Time per request:       0.810 [ms] (mean, across all concurrent requests)
  95%     52
"""


class RunnerTests(unittest.TestCase):
    def test_django_pool_is_per_worker_and_uses_optional_psycopg3(self):
        with patch.object(runner.subprocess, "Popen") as launch, \
             patch.object(runner, "wait_for"), patch.object(runner.os, "killpg"):
            with runner.server("django", 0, Path("unused"), 8,
                               postgres_url="postgresql://localhost/bench", django_pool_size=4):
                args, kwargs = launch.call_args
                self.assertIn("--exact", args[0])
                self.assertIn("--locked", args[0])
                self.assertEqual(args[0][args[0].index("--extra") + 1], "pool")
                self.assertEqual(kwargs["env"]["BENCHMARK_DJANGO_POOL_SIZE"], "4")

    def test_django_pool_settings_keep_request_cleanup_and_bound_connections(self):
        parser = Mock(return_value={"dbname": "bench", "host": "localhost", "options": "-csearch_path=bench"})
        with patch.dict(os.environ, {"BENCHMARK_POSTGRES_URL": "postgresql://localhost/bench",
                                     "BENCHMARK_DJANGO_POOL_SIZE": "4"}), \
             patch.dict("sys.modules", {"psycopg.conninfo": SimpleNamespace(conninfo_to_dict=parser)}):
            config = runpy.run_path(str(Path(__file__).parent / "django/config/settings.py"))["DATABASES"]["default"]
        self.assertEqual(config["CONN_MAX_AGE"], 0)
        self.assertEqual(config["NAME"], "bench")
        self.assertEqual(config["OPTIONS"]["options"], "-csearch_path=bench")
        self.assertEqual(config["OPTIONS"]["pool"], {"min_size": 4, "max_size": 4, "timeout": 30})

    def test_django_pool_rejects_sqlite(self):
        with self.assertRaisesRegex(ValueError, "requires PostgreSQL"):
            with runner.server("django", 0, Path("fixture.sqlite"), 8, django_pool_size=4):
                self.fail("pool must be rejected before starting a server")

    def test_postgres_server_preserves_full_url_and_binary_selection(self):
        url = "postgresql://user@localhost/benchmark?sslmode=require&options=-csearch_path%3Dbench"
        with patch.object(runner.subprocess, "Popen") as launch, \
             patch.object(runner, "wait_for"), patch.object(runner.os, "killpg"):
            with runner.server("che-rest", 0, Path("unused"), 8, postgres_url=url,
                               rust_binary=Path("/postgres-release/cli_fullstack"), rust_pool_size=32):
                args, kwargs = launch.call_args
                self.assertEqual(args[0], ["/postgres-release/cli_fullstack"])
                self.assertEqual(kwargs["env"]["BENCHMARK_POSTGRES_URL"], url)
                config = (kwargs["cwd"] / "app.toml").read_text()
                self.assertIn("url = " + json.dumps(url), config)
                self.assertIn("max_connections = 32", config)

    def test_sqlite_server_does_not_inherit_postgres_url(self):
        with patch.dict(os.environ, {"BENCHMARK_POSTGRES_URL": "postgresql://wrong/target",
                                     "BENCHMARK_DJANGO_POOL_SIZE": "32"}), \
             patch.object(runner.subprocess, "Popen") as launch, \
             patch.object(runner, "wait_for"), patch.object(runner.os, "killpg"):
            with runner.server("che-rest", 0, Path("/fixture.sqlite"), 8):
                self.assertNotIn("BENCHMARK_POSTGRES_URL", launch.call_args.kwargs["env"])
                self.assertNotIn("BENCHMARK_DJANGO_POOL_SIZE", launch.call_args.kwargs["env"])
                config = (launch.call_args.kwargs["cwd"] / "app.toml").read_text()
                self.assertIn("sqlite:///fixture.sqlite", config)
                self.assertIn("max_connections = 4", config)

    def test_latency_is_not_overwritten_by_throughput_time(self):
        values = runner.parse_ab(AB_OUTPUT)
        self.assertEqual(values["requests_per_second"], 1234.56)
        self.assertEqual(values["mean_latency_ms"], 40.5)
        self.assertEqual(values["p95_latency_ms"], 52)
        self.assertEqual(values["non_2xx_responses"], 0)

    def test_incomplete_output_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            runner.parse_ab("Failed requests: 0")

    def test_http_errors_and_failed_requests_invalidate_measurements(self):
        for output in (AB_OUTPUT + "Non-2xx responses: 100\n",
                       AB_OUTPUT.replace("Failed requests:        0", "Failed requests: 5")):
            with self.subTest(output=output), patch.object(runner.subprocess, "run") as run:
                run.return_value = subprocess.CompletedProcess([], 0, output, "")
                with self.assertRaisesRegex(RuntimeError, "invalid benchmark"):
                    runner.benchmark("http://localhost/", 100, 10)

    def test_live_broken_endpoint_fails_without_startup_timeout(self):
        process = Mock()
        process.poll.return_value = None
        error = urllib.error.HTTPError("http://localhost/", 500, "error", {}, None)
        error.read = Mock(return_value=b"missing column")
        with patch.object(runner, "request", side_effect=error):
            with self.assertRaisesRegex(RuntimeError, "HTTP 500: missing column"):
                runner.wait_for("http://localhost/", process)

    def test_outdated_database_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "benchmark.sqlite"
            with sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE tasks_task (id INTEGER PRIMARY KEY)")
            with self.assertRaisesRegex(RuntimeError, "outdated benchmark database"):
                runner.validate_database(database, 500)

    def test_missing_database_is_not_created(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "missing.sqlite"
            with self.assertRaises(sqlite3.OperationalError):
                runner.validate_database(database, 500)
            self.assertFalse(database.exists())

    def test_missing_retrieve_id_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "benchmark.sqlite"
            with sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE tasks_task (id, author_id, assignee_id, name, status, created_at, updated_at)")
                connection.execute("INSERT INTO tasks_task (id, created_at, updated_at) VALUES (1, ?, ?)",
                                   ("2026-01-01T00:00:00.000000000Z",) * 2)
            self.assertEqual(runner.validate_database(database, 1), 1)
            with self.assertRaisesRegex(RuntimeError, "task 500 does not exist"):
                runner.validate_database(database, 500)


class PrepareTests(unittest.TestCase):
    def test_mixed_datetime_formats_are_rejected_before_benchmarking(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "benchmark.sqlite"
            with sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE tasks_task (id, author_id, assignee_id, name, status, created_at, updated_at)")
                connection.execute("INSERT INTO tasks_task (id, created_at, updated_at) VALUES (1, ?, ?)",
                                   ("2026-01-01T00:00:00Z", "2026-01-01T00:00:00.000000000Z"))
            with self.assertRaisesRegex(RuntimeError, "outdated benchmark datetime format"):
                runner.validate_database(database, 1)

    def test_existing_database_requires_force(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "benchmark.sqlite"
            database.write_bytes(b"original database")
            with patch.object(prepare.SCHEMA_BINARY.__class__, "is_file", return_value=True), \
                 patch.object(prepare, "seed") as seed, \
                 patch("sys.argv", ["prepare.py", "--database", str(database)]), \
                 patch("sys.stderr"):
                with self.assertRaises(SystemExit):
                    prepare.main()
                seed.assert_not_called()
            self.assertEqual(database.read_bytes(), b"original database")

    def test_failed_seed_preserves_existing_database_even_with_force(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "benchmark.sqlite"
            database.write_bytes(b"original database")
            with patch.object(prepare.SCHEMA_BINARY.__class__, "is_file", return_value=True), \
                 patch.object(prepare, "seed", side_effect=RuntimeError("seed failed")), \
                 patch("sys.argv", ["prepare.py", "--database", str(database), "--force"]):
                with self.assertRaisesRegex(RuntimeError, "seed failed"):
                    prepare.main()
            self.assertEqual(database.read_bytes(), b"original database")

    @unittest.skipUnless(prepare.SCHEMA_BINARY.is_file(), "build benchmark_schema first")
    def test_fixture_uses_current_schema_and_covers_enum_and_nullable_relation(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "benchmark.sqlite"
            prepare.seed(database, 6)
            self.assertEqual(runner.validate_database(database, 1), 6)
            with sqlite3.connect(database) as connection:
                self.assertEqual({row[0] for row in connection.execute("SELECT DISTINCT status FROM tasks_task")},
                                 {"draft", "in_progress", "done"})
                self.assertEqual(connection.execute("SELECT COUNT(*) FROM tasks_task WHERE assignee_id IS NULL").fetchone()[0], 3)
                self.assertEqual(connection.execute("PRAGMA foreign_key_check").fetchall(), [])
                for dates in connection.execute("SELECT created_at, updated_at FROM tasks_task"):
                    for value in dates:
                        self.assertEqual(len(value), 30)
                        self.assertTrue(value.endswith(".000000000Z"))


if __name__ == "__main__":
    unittest.main()
