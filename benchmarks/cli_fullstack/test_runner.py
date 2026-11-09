import importlib.util
import sqlite3
import subprocess
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import Mock, patch


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
                connection.execute("INSERT INTO tasks_task (id) VALUES (1)")
            self.assertEqual(runner.validate_database(database, 1), 1)
            with self.assertRaisesRegex(RuntimeError, "task 500 does not exist"):
                runner.validate_database(database, 500)


class PrepareTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
