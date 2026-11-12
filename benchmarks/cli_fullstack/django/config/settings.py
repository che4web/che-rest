import os
from pathlib import Path


BASE_DIR = Path(__file__).resolve().parent.parent
SECRET_KEY = "benchmark-only"
DEBUG = False
ALLOWED_HOSTS = ["127.0.0.1", "localhost"]
ROOT_URLCONF = "config.urls"
MIDDLEWARE = []
INSTALLED_APPS = ["tasks"]
DATABASES = {
    "default": {
        "ENGINE": "django.db.backends.sqlite3",
        "NAME": os.environ.get("BENCHMARK_DATABASE", str(BASE_DIR / "benchmark.sqlite")),
        "CONN_MAX_AGE": 0,
    }
}
if os.environ.get("BENCHMARK_POSTGRES_URL"):
    # libpq parses the complete URL, preserving options/search_path and TLS.
    pool_size = int(os.environ.get("BENCHMARK_DJANGO_POOL_SIZE", "0"))
    if pool_size < 0:
        raise ValueError("BENCHMARK_DJANGO_POOL_SIZE must be nonnegative")
    if pool_size:
        from psycopg.conninfo import conninfo_to_dict
        postgres_options = conninfo_to_dict(os.environ["BENCHMARK_POSTGRES_URL"])
        postgres_options["pool"] = {"min_size": pool_size, "max_size": pool_size, "timeout": 30}
    else:
        import psycopg2.extensions
        postgres_options = psycopg2.extensions.parse_dsn(os.environ["BENCHMARK_POSTGRES_URL"])
    DATABASES["default"] = {
        "ENGINE": "django.db.backends.postgresql",
        "NAME": postgres_options.pop("dbname"),
        "OPTIONS": postgres_options,
        "CONN_MAX_AGE": 0,
    }
USE_TZ = True
DEFAULT_AUTO_FIELD = "django.db.models.BigAutoField"
