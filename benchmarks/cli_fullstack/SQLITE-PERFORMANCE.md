# SQLite read-performance investigation

Local investigation recorded on 2026-11-12. This is a workload-specific report,
not a framework ranking or a promise of production throughput.

**Status: documented; performance changes deferred.** The compile-time flags,
process initialization and pool defaults discussed below have not been changed
in the framework. Experimental executables were separate diagnostic builds. The
canonical UTC datetime storage work is independent of these experiments.

## Workload and comparison limits

- 10,000 tasks, one author, nullable assignees, enum fields and canonical UTC TEXT
  timestamps. The sorted endpoint returns 50 tasks and a count.
- Release Rust build; HTTP keep-alive; 200 warmup requests. Servers run one at a
  time against the same disposable fixture for each comparison.
- Full framework comparison: 10,000 requests per scenario, concurrency 50,
  eight Python workers and a four-connection Rust pool. Each indexed/unindexed
  configuration was measured once. JSON parity passed for all four servers.
- Follow-up HTTP controls: 2,000 requests per measurement, concurrency 1 and/or
  50, two repetitions with reversed configuration order. Tables report arithmetic
  mean requests/second. Measurements are not CPU-budget-normalized.
- Synchronous FastAPI endpoints can use multiple threadpool threads and SQLite
  connections per worker. A separate one-sync-endpoint-at-a-time-per-worker
  control was measured; the ordinary runner does not apply that restriction.
- Identical-SQL timings used uncached statement preparation, ten warmups and the
  median of five batches of 100 executions. Native measurements distinguished
  stepping from materializing all returned values; their costs were comparable
  for this workload. No HTTP or model/JSON conversion was involved.
- Controlled native build-flag scaling tests used 480 total queries across
  one/four/eight threads with independent connections, three repetitions, and
  median throughput. A separate earlier process/thread control used 800 queries.
- Linux host with 16 available logical CPUs. Rust bundled SQLite was 3.50.2
  (`libsqlite3-sys` 0.35.0 / `rusqlite` 0.37.0); FastAPI's actual `uv` environment
  used Python 3.13.13 and SQLite 3.50.4. An additional Rust system-SQLite control
  used 3.54.1. The system Python used to run the harness was not the FastAPI
  application's interpreter. Record actual executable versions when repeating.

All reported HTTP runs had zero failed requests and zero non-2xx responses.
There was visible run-to-run variation. Compare paired controls within a series;
do not treat control values from different series as one continuous baseline.

## Full HTTP comparison: effect of an ordinary index

Requests/second, rounded; larger is better. Both fixtures used canonical datetime
TEXT. The indexed fixture was a copy with the same `created_at` index available
to every server. No framework-specific index advantage was introduced.

### Without the datetime index

| Framework | Sorted list | Filtered list | Retrieve |
| --- | ---: | ---: | ---: |
| che-rest | 301 | 2,559 | 31,329 |
| FastAPI + sqlite3 | 922 | 1,077 | 6,514 |
| FastAPI + SQLAlchemy | 831 | 795 | 4,489 |
| Django ORM | 712 | 798 | 2,369 |

### With the datetime index

| Framework | Sorted list | Filtered list | Retrieve |
| --- | ---: | ---: | ---: |
| che-rest | 4,885 | 2,546 | 32,358 |
| FastAPI + sqlite3 | 1,401 | 1,174 | 8,806 |
| FastAPI + SQLAlchemy | 910 | 865 | 4,393 |
| Django ORM | 906 | 839 | 2,381 |

Without the index, the inspected joined-query plan included `SCAN` and
`USE TEMP B-TREE FOR ORDER BY`. With it, the plan scanned the datetime index and
did not need that temporary sorting structure. The roughly 16x che-rest sorted
list improvement is an indexing result, not a datetime-format before/after test.
The datetime index does not serve the `name__contains` scenario.

## Correction: the HTTP slowdown was not a parsing collation

The initial investigation attributed the sorted HTTP list's slowdown to repeated
RFC 3339 parsing in the old `che_datetime` collation. That diagnosis was incorrect
for this endpoint: `FilterSet` ordering called `apply_order`, which reconstructed
the field with `ModelField::new` and therefore did not preserve its datetime
marker. Its older ORDER BY did not request `che_datetime`.

Direct ORM timestamp-field queries did use that collation. Their isolated
comparator benchmark demonstrates its cost, but cannot explain this REST path's
historical HTTP performance. Similarly, comparing an old saved result with a
new canonical fixture is not a controlled measurement of the storage change.
Capture the actual SQL, parameters and plan before attributing an endpoint's
performance to a compiler feature.

## SQL projection and count costs

The captured ORM list selected all task columns and all seven author columns,
including `password_hash` and flags omitted from the JSON. The narrow control
selected the same task columns and only `author.username`, using `author_id`
already present in the task. Both retained the same JOIN, sort, LIMIT and OFFSET.

Median milliseconds/query, rounded:

| Query | Rust, bundled SQLite | Python, application SQLite |
| --- | ---: | ---: |
| Sorted list, full author projection | 4.34 | 4.25 |
| Sorted list, narrow author projection | 3.44 | 3.25 |
| COUNT with JOIN | 0.190 | 0.245 |
| COUNT around the ORM joined subquery | 0.200 | 0.245 |
| COUNT of tasks without JOIN | 0.0044 | 0.0055 |

The system-SQLite Rust control took about 4.05 ms for the wide list. The engines
and compiler options differed, so this does not prove driver equivalence for all
workloads, but it does not support a several-fold Rust-driver penalty here.

Narrowing the projection saved roughly 20-25%. Removing a genuinely unnecessary
count JOIN could save about 0.2 ms. The wrapped count was not a costly extra
materialization: its inspected plan matched the direct joined count. Neither
factor alone explained the parallel HTTP gap. Removing JOINs is not generally
safe when they affect membership, related filters, duplicates or authorization.

## Parallelism is not just pool size

Unindexed HTTP sorted list at concurrency 50; mean of two runs:

| Configuration | Requests/second |
| --- | ---: |
| Rust, pool 4 | 322 |
| Rust, pool 8 | 180 |
| FastAPI, 4 workers, normal threadpool | 750 |
| FastAPI, 8 workers, normal threadpool | 767 |
| FastAPI, 4 workers, one sync endpoint per worker | 281 |
| FastAPI, 8 workers, one sync endpoint per worker | 393 |

The Python restriction was diagnostic, not a new default. It demonstrates why
worker counts must not be equated to connection counts. These settings also do
not equalize CPU or request scheduling between processes and threads.

An earlier pure-rusqlite control reproduced the scaling problem without the ORM:
about 218 / 334 / 186 queries/second for one / four / eight threads of one
process, versus about 580 / 780 for four / eight separate processes. These were
medians of three 800-query runs, separate from the following build-flag series.
The problem is therefore not explained solely by Tokio, JSON serialization or
the ORM pool implementation.

## Confirmed contributor: shared page-cache management

The investigated bundled build enables `SQLITE_ENABLE_MEMORY_MANAGEMENT` in
`libsqlite3-sys`. SQLite's `pcache1Init` then forces `separateCache = 0` and uses a
shared cache-management group protected by `SQLITE_MUTEX_STATIC_LRU`. This is not
the application-level SQLite shared-cache feature and not a database write lock.
Concurrent read/sort activity can contend on that process-wide management mutex.

Isolated builds changed only this compile-time flag using the supported build
override `LIBSQLITE3_FLAGS=-USQLITE_ENABLE_MEMORY_MANAGEMENT`. The diagnostic
binary checked the resulting compile option; SQLite thread safety stayed enabled.

Native wide-query throughput, median of three 480-query runs:

| Threads in one process | Build option enabled | Build option disabled |
| --- | ---: | ---: |
| 1 | 198 | 221 |
| 4 | 337 | 575 |
| 8 | 173 | 534 |

The corresponding HTTP A/B series changed neither SQL nor model/JSON handling:

| Rust pool | Original build | Option disabled |
| --- | ---: | ---: |
| 4 | 270 requests/s | 401 requests/s |
| 8 | 159 requests/s | 429 requests/s |

This confirms a substantial contributor, not a universal explanation of every
SQLite performance difference. Default pool sizes and bundled-driver settings
have not been changed as a result of this investigation.

## Second contributor: allocation statistics

SQLite allocation statistics have another shared mutex. Disabling only memory
statistics before initialization did not materially improve the original native
build: with eight threads, medians were about 186 queries/s with statistics and
181 without. The page-cache bottleneck remained.

After disabling `SQLITE_ENABLE_MEMORY_MANAGEMENT`, a new paired series showed
the previously masked cost. Native medians from three 480-query runs:

| Threads | Statistics enabled | Statistics disabled |
| --- | ---: | ---: |
| 1 | 234 | 243 |
| 4 | 717 | 824 |
| 8 | 704 | 1,146 |

In a separate HTTP A/B series, both builds already had the page-cache option
disabled. Mean throughput over two runs at concurrency 50:

| Rust pool | Statistics enabled | Statistics disabled |
| --- | ---: | ---: |
| 4 | 497 requests/s | 517 requests/s |
| 8 | 497 requests/s | 885 requests/s |

These are separate paired series: the 429 and 497 requests/s controls must not be
treated as the same measurement. The initialization experiment used
`SQLITE_CONFIG_MEMSTATUS=0` before opening SQLite or starting application work;
it did not disable thread safety or change stored data.

## Current guidance and deferred work

- Index frequent ordering/range predicates and verify the actual query plan.
  Normalization does not create new indexes; production indexes belong in
  explicit compiled migrations.
- Measure pool size. More connections can reduce throughput for this build and
  workload; eight is not automatically better than four.
- Keep canonical datetime correctness/migration requirements separate from this
  concurrency issue. Normalizing data does not remove full scans or mutex costs.
- Do not ship the diagnostic flag overrides as an undocumented fix. Disabling
  `SQLITE_ENABLE_MEMORY_MANAGEMENT` affects global cache-memory release and soft
  memory-management behavior. Disabling statistics affects accounting and related
  controls and must be coordinated before process-wide SQLite initialization.
- A dependency's Cargo config is not a downstream application build contract.
  Distribution of a build option and an explicit initialization API need separate
  design, memory-behavior tests, full backend/downstream checks and approval.
- Projection/count optimizations must preserve materialized-model contracts,
  related filters, scope authorization and result cardinality.

The ordinary runner has no pool-size, private-page-cache or memory-statistics
switch. Diagnostic harnesses were temporary and are not a shipped feature. Their
local artifacts were in `/tmp/opencode/datetime-profile-tH6p6OOl/`; the complete
indexed/unindexed comparison was in `/tmp/opencode/python-comparison-w4uwbnsx/`.
These paths are session-local provenance, not permanent download links. The
tables and conditions above preserve the conclusions without depending on those
files. The checked-in `results.json` remains a distinct historical benchmark.
