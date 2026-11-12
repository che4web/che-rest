# PostgreSQL HTTP comparison: 2026-11-12

These are measurements of four specific local applications, not a general
framework ranking. che-rest leads detail retrieval in this configuration but
trails all three Python applications on list endpoints. A datetime index makes
little difference to measured HTTP throughput on this 10,000-row workload.

## Conditions

- Intel Core i7-1260P, 16 logical CPUs; Linux x86-64. Host not CPU-isolated.
- PostgreSQL 18.6, local TCP without TLS, UTF-8/C locale, timezone UTC.
- Default PostgreSQL shared_buffers 128 MB, work_mem 4 MB, max_connections 100,
  fsync **on**. No experimental database initialization settings.
- 10,000 tasks, one author, all three status values, alternating nullable
  assignees. Equal parsed TaskSerializer JSON, eager author loading, public reads
  without request-time authentication database lookups.
- Rust release build, rustc 1.92.0; ORM `618bbba`, REST `9168387`, plus the
  uncommitted benchmark adapters identified by source hashes in results.json.
- Python 3.13.13; FastAPI 0.115.6, SQLAlchemy 2.0.38, Django 5.1.6,
  Uvicorn 0.34.0, psycopg2-binary 2.9.11. See checked-in project lock files.
- Apache Bench, HTTP keep-alive, concurrency 50, 10,000 measured requests and
  500 warmup requests per scenario. Three repeats, rotating server order.
- Servers run sequentially, never concurrently. Warm-cache read-only workload.
- Two variants on the same seed: no datetime index, then an ordinary
  `tasks_task(created_at)` index. ANALYZE runs after seeding/index creation.

Exact parsed JSON parity passed for list, filtered list, pagination, empty
result and detail, before both index variants. **All 72 measured runs passed:**
720,000 measured requests, zero failed requests and zero non-2xx responses.
Warmups add 36,000 requests and also passed. Servers and the disposable database
cluster were stopped when the script completed.

## Median throughput

Requests/second, median of three runs (higher is better):

| Application | Sorted list, no index | Sorted list, index | Filtered list, no index | Filtered list, index | Detail, no index | Detail, index |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| che-rest | 314.41 | 314.04 | 625.83 | 628.02 | 4,496.15 | 4,453.31 |
| FastAPI / direct psycopg2 | 853.33 | 861.45 | 1,142.41 | 1,140.51 | 1,828.55 | 1,825.84 |
| FastAPI / SQLAlchemy | 1,165.76 | 1,164.26 | 1,434.87 | 1,416.85 | 2,868.41 | 2,798.09 |
| Django ORM | 730.59 | 748.73 | 919.30 | 912.52 | 1,301.90 | 1,300.77 |

Scenarios:

- Sorted list: `GET /api/tasks/?limit=50&ordering=-created_at`.
- Filtered list: `GET /api/tasks/?limit=50&name__contains=task` (matches all rows,
  returns 50; default ordering by ID).
- Detail: `GET /api/tasks/500/`.

## Latency

Without the datetime index, median mean latency / median per-run p95, in ms
(lower is better). Mean latency is concurrency divided by throughput, not the
Apache Bench across-concurrency time. The median of three p95 values is not a
pooled percentile across all requests. Apache Bench reports integer-ms p95.

| Application | Sorted list, mean / p95 | Filtered list, mean / p95 | Detail, mean / p95 |
| --- | ---: | ---: | ---: |
| che-rest | 159.03 / 163 | 79.89 / 83 | 11.12 / 12 |
| FastAPI / direct psycopg2 | 58.59 / 83 | 43.77 / 61 | 27.34 / 38 |
| FastAPI / SQLAlchemy | 42.89 / 72 | 34.85 / 50 | 17.43 / 26 |
| Django ORM | 68.44 / 97 | 54.39 / 76 | 38.41 / 52 |

The indexed che-rest means/p95 are 159.22/163 ms for sorted list,
79.62/82 ms for filtered list and 11.23/12 ms for detail. Full indexed latency
results for all applications are in summary.json.

## Interpretation and limits

- For detail without the index, che-rest delivered about 1.57x SQLAlchemy,
  2.46x direct FastAPI and 3.45x Django throughput.
- For sorted lists, SQLAlchemy delivered about 3.71x che-rest throughput;
  for filtered lists it delivered about 2.29x. There is no uniform Rust win.
- che-rest's sorted-list range over three unindexed runs was 314.18-316.47 rps;
  filtered-list range was 625.16-626.48 rps. The repeated list gap was not a
  single anomalous run.
- The representative direct-SQL EXPLAIN snapshot changed from a sort to a
  backward index scan. Its single list SELECT went from 7.784 ms to 0.210 ms,
  **but HTTP throughput barely changed**. These are isolated, non-concurrent
  plan snapshots, not per-framework SQL profiles or proof of the HTTP bottleneck.
- che-rest's list bottleneck remains unexplained here. Pool waiting, actual
  generated SQL/counts, decoding, serialization and transport need profiling;
  this benchmark alone cannot assign the gap to PostgreSQL or the ORM.
- Connection budgets differ: che-rest has four connections total; SQLAlchemy
  has four **per Uvicorn worker**, up to 32 across eight workers. Direct FastAPI
  and Django open a connection per request. Their numbers include connection
  establishment; the pooled applications amortize that cost. A future equal-pool
  or pooled direct-driver experiment would answer a different question.
- Rust uses one Tokio multithreaded process (default worker count); Python uses
  eight worker processes plus synchronous endpoint/thread behavior. This is not
  an equal-core allocation or an ORM-only benchmark. Count-query JOINs and author
  projections also differ between implementations despite equal response JSON.
- The host was not CPU-isolated; load averages before/after are recorded. Three
  repetitions are descriptive, not confidence intervals or production capacity.
- These PostgreSQL HTTP numbers must not be treated as a direct comparison with
  historical SQLite or ORM-only reports that used different builds/settings.

## Saved artifacts and reproduction

- [results.json](results.json): all 72 measurements, settings, source/binary hashes.
- [summary.json](summary.json): medians by application/scenario/index variant.
- [ab/](ab/): all 72 full Apache Bench reports plus 72 warmup reports.
- [plans-unindexed.json](plans-unindexed.json) and
  [plans-indexed.json](plans-indexed.json): representative direct SQL plus direct
  and joined count plans. Not captured SQL traces from all four applications.
- [run.log](run.log): parity confirmations and sequential measurement output.

Reproduction commands are in the parent
[benchmark guide](../../README.md#postgresql-http-comparison). Use a new output
directory. No database files or local connection URLs are included here.
