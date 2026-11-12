# PostgreSQL HTTP comparison: che-rest pool 32

Follow-up to the [pool-4 baseline](../2026-11-12/README.md), using the **same Rust
release binary** with `max_connections = 32` in the temporary benchmark config.
No application/library defaults or PostgreSQL tuning settings were changed.
Python applications and connection models were unchanged and measured again.

## Method

Same workload as the baseline: PostgreSQL 18.6, 10,000 tasks, HTTP keep-alive,
concurrency 50, eight Python workers, three repetitions per scenario/index
variant. Each measurement has 500 warmup and 10,000 measured requests. Servers
run sequentially; order rotates between repetitions. There is no datetime index
first, then an ordinary `created_at` index is added to the same fixture.

Parsed JSON parity passed on all five paths for both index variants. All
**72 measurements / 720,000 measured requests passed**, with zero failed requests
and zero non-2xx responses. All warmups passed too. The cluster and servers were
stopped after completion. This is a separate run on a non-CPU-isolated host,
not an interleaved randomized pool-size experiment.

## che-rest: pool 4 versus pool 32

Median of three runs; throughput in requests/second:

| Index | Scenario | Pool 4 | Pool 32 | Throughput ratio | Mean latency, 4 -> 32 (ms) | p95, 4 -> 32 (ms) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| None | Sorted list | 314.41 | 1,559.81 | 4.96x | 159.03 -> 32.06 | 163 -> 40 |
| None | Filtered list | 625.83 | 3,204.47 | 5.12x | 79.89 -> 15.60 | 83 -> 20 |
| None | Detail | 4,496.15 | 18,356.64 | 4.08x | 11.12 -> 2.72 | 12 -> 3 |
| created_at | Sorted list | 314.04 | 1,571.83 | 5.01x | 159.22 -> 31.81 | 163 -> 39 |
| created_at | Filtered list | 628.02 | 3,201.70 | 5.10x | 79.62 -> 15.62 | 82 -> 20 |
| created_at | Detail | 4,453.31 | 18,423.20 | 4.14x | 11.23 -> 2.71 | 12 -> 3 |

p95 columns are medians of the three per-run p95 measurements, not pooled
percentiles. Apache Bench's p95 has integer-millisecond resolution.

## All applications in the pool-32 series

Median requests/second; che-rest has 32 connections total, SQLAlchemy still has
four per worker (up to 32 total). Other Python applications still open a new
connection per request.

| Application | Sorted list, no index | Sorted list, index | Filtered list, no index | Filtered list, index | Detail, no index | Detail, index |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| che-rest | 1,559.81 | 1,571.83 | 3,204.47 | 3,201.70 | 18,356.64 | 18,423.20 |
| FastAPI / direct psycopg2 | 821.58 | 841.62 | 1,068.97 | 1,064.36 | 1,744.07 | 1,733.68 |
| FastAPI / SQLAlchemy | 1,154.62 | 1,146.65 | 1,427.53 | 1,412.21 | 2,938.71 | 2,908.98 |
| Django ORM | 735.52 | 730.68 | 850.33 | 855.57 | 1,252.52 | 1,244.36 |

## Interpretation

- Raising only che-rest's pool ceiling from 4 to 32 improved measured throughput
  about 4-5x. The small pool was a substantial concurrency constraint in this
  workload; this does not identify the reason for each connection's service time.
- In this series che-rest leads all three endpoints. Without the datetime index,
  its throughput is about 1.35x SQLAlchemy for sorted lists, 2.24x for filtered
  lists and 6.25x for detail.
- The datetime index still makes little difference to end-to-end HTTP throughput.
  Do not infer from this that indexing is unnecessary for other data volumes,
  filters, SQL projections or concurrency levels.
- Equal **maximum** pooled connections for che-rest/SQLAlchemy is a closer
  comparison, but actual open/busy counts were not instrumented. Rust has one
  global pool, SQLAlchemy has eight separate worker pools; CPU/process models,
  SQL projections/count JOINs and serialization still differ. Direct FastAPI
  and Django are unpooled, so their numbers include connection establishment.
- Unchanged Python controls moved between approximately -7.5% and +4% relative
  to their baseline medians. Environmental/run variability remains visible;
  the che-rest gain is much larger, but these are descriptive local results,
  not a general framework ranking or an optimal production pool recommendation.
- A higher pool ceiling consumes more database resources. This experiment does
  not establish that 32 is optimal, or that additional application processes
  should each receive 32 connections on a shared PostgreSQL server.

## Artifacts and reproduction

- [results.json](results.json): 72 raw measurements and full environment/source
  metadata, explicitly recording `rust_pool_size: 32`.
- [summary.json](summary.json): throughput and latency medians.
- [ab/](ab/): 72 full measured reports plus 72 warmup reports.
- [plans-unindexed.json](plans-unindexed.json), [plans-indexed.json](plans-indexed.json):
  representative direct SQL and count-query EXPLAIN snapshots, not SQL traces
  from all four implementations.
- [run.log](run.log): parity confirmations and measurement output.

Build the PostgreSQL binaries as documented in the parent
[benchmark guide](../../README.md#postgresql-http-comparison), then:

```bash
uv run --project benchmarks/cli_fullstack/fastapi \
  python benchmarks/cli_fullstack/postgres.py \
  --rust-bin-dir /tmp/opencode/che-rest-http-pg-target/release \
  --rust-pool-size 32 \
  --output-dir /tmp/opencode/NEW-postgres-pool32-results
```

Use a new output directory. No database files or local connection URLs are saved
in this repository report.
