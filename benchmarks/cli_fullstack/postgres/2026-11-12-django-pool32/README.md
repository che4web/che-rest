# PostgreSQL HTTP comparison: pooled Django

Follow-up to the [che-rest pool-32 series](../2026-11-12-pool32/README.md).
Django now uses its native psycopg 3 connection pool: **four connections per
worker, eight workers, maximum 32 total**. che-rest remains at 32 total;
SQLAlchemy remains at four per worker (32 maximum). Direct FastAPI is unchanged
and still opens a connection per request.

This changes both Django's connection lifecycle and driver: psycopg2 2.9.11
without pooling -> psycopg 3.2.13 with psycopg_pool 3.3.3. It is not a pure
pool-only A/B experiment. The old unpooled path remains reproducible using the
default dependency set; the runner exactly syncs locked extras for each launch.

## Conditions and verification

PostgreSQL 18.6, 10,000 tasks, local TCP without TLS, same Rust release binary,
HTTP keep-alive, concurrency 50, 500 warmup plus 10,000 measured requests per
scenario, three repetitions, with and without a datetime index. Applications run
one at a time, each Python application with eight workers; server order rotates
between repetitions.
See the [original report](../2026-11-12/README.md) for the schema, hardware and
methodology. Full settings and hashes are in results.json.

Django keeps `CONN_MAX_AGE=0`: request cleanup returns the connection to the
shared per-process pool. `min_size=max_size=4`, acquisition timeout 30 seconds.
Django's default disabled automatic prepared statements remain unchanged.

Exact parsed JSON parity passed for five paths before both index variants.
**72 measurements / 720,000 measured requests passed with zero failed requests
and zero non-2xx responses.** All 36,000 warmup requests also passed. Servers
and the disposable PostgreSQL cluster were stopped after the run.

## Django before and after

Median of three runs. The previous series used psycopg2 without a pool; this
series uses psycopg 3 with a pool. Throughput is requests/second.

| Index | Scenario | Before | Pooled | Ratio | Mean latency, before -> pooled (ms) | p95, before -> pooled (ms) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| None | Sorted list | 735.52 | 1,095.89 | 1.49x | 67.98 -> 45.63 | 94 -> 72 |
| None | Filtered list | 850.33 | 1,256.66 | 1.48x | 58.80 -> 39.79 | 82 -> 58 |
| None | Detail | 1,252.52 | 1,386.81 | 1.11x | 39.92 -> 36.05 | 55 -> 54 |
| created_at | Sorted list | 730.68 | 1,089.98 | 1.49x | 68.43 -> 45.87 | 95 -> 72 |
| created_at | Filtered list | 855.57 | 1,249.22 | 1.46x | 58.44 -> 40.03 | 82 -> 60 |
| created_at | Detail | 1,244.36 | 1,377.75 | 1.11x | 40.18 -> 36.29 | 55 -> 55 |

p95 values are medians of three per-run percentiles, not a pooled percentile.
Apache Bench reports p95 at integer-ms resolution.

## All applications in this series

Median requests/second:

| Application | Sorted list, no index | Sorted list, index | Filtered list, no index | Filtered list, index | Detail, no index | Detail, index |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| che-rest, pool 32 | 1,610.19 | 1,603.75 | 3,332.31 | 3,343.43 | 18,397.37 | 18,379.55 |
| FastAPI / direct psycopg2, unpooled | 868.76 | 848.88 | 1,126.21 | 1,120.88 | 1,806.85 | 1,796.42 |
| FastAPI / SQLAlchemy, max 32 pooled | 1,162.48 | 1,159.24 | 1,417.66 | 1,405.47 | 2,885.00 | 2,973.57 |
| Django / psycopg 3, max 32 pooled | 1,095.89 | 1,089.98 | 1,256.66 | 1,249.22 | 1,386.81 | 1,377.75 |

## Conclusions and limitations

- The pooled/psycopg-3 Django configuration improves list throughput about
  46-49% and detail about 11% relative to the preceding unpooled series.
- The small detail gain does not support explaining the entire original Django
  gap by connection establishment alone. This run does not separately quantify
  ORM, ASGI/sync adaptation, serialization, driver or pool-management overhead.
- On unindexed lists Django now reaches about 94% of SQLAlchemy's sorted-list
  throughput and 89% of its filtered-list throughput. Detail remains about 48%.
- che-rest still leads these specific configurations. Equal configured maximum
  connections do not make CPU/process models or SQL/projection behavior equal;
  pool activity and internal wait times were not instrumented.
- The index again has little effect on measured HTTP throughput at this data
  size. This is not evidence against indexing for larger or different workloads.
- Unchanged controls moved by roughly -2% to +6% between series. The host is not
  CPU-isolated and comparisons are between sequential series, not randomized
  interleaved A/B trials. Do not treat these as production capacity guarantees.

## Artifacts and reproduction

- [results.json](results.json): all 72 measurements; Django driver/pool versions,
  per-worker and total limits, environment/source hashes.
- [summary.json](summary.json): throughput/latency medians.
- [ab/](ab/): 72 measured and 72 warmup Apache Bench outputs.
- [plans-unindexed.json](plans-unindexed.json), [plans-indexed.json](plans-indexed.json):
  representative direct SQL/count plans, not actual traces from all applications.
- [run.log](run.log): parity confirmations and measurement output.

After building the PostgreSQL release binaries as described in the parent guide:

```bash
uv run --project benchmarks/cli_fullstack/fastapi \
  python benchmarks/cli_fullstack/postgres.py \
  --rust-bin-dir /tmp/opencode/che-rest-http-pg-target/release \
  --rust-pool-size 32 --django-pool-size 4 --workers 8 \
  --output-dir /tmp/opencode/NEW-django-pooled-results
```

The output directory must be new. No database files or local connection URLs
are stored in this report.
