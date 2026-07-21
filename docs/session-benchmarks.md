# Session and Template Benchmarks

Measurements below were collected on 2026-07-21 on the same macOS host with
managed-local PostgreSQL 18. They are evidence for defaults, not portable
performance promises.

## Template YAGNI gate

A synthetic migration created 200 tables and 200 indexes. Three warm fresh
runs measured database creation plus the migration; three template runs cloned
the same migrated schema.

| Path | Run 1 | Run 2 | Run 3 | Median |
| --- | ---: | ---: | ---: | ---: |
| Fresh create + migration | 2.457s | 2.514s | 1.539s | 2.457s |
| Template clone | 1.045s | 1.137s | 0.877s | 1.045s |

The initial template cost was 2.032 seconds to create the source, 2.210 seconds
to migrate it, and 1.819 seconds to write the template. Four concurrent clones
then completed successfully with four unique database IDs in 0.753 seconds of
wall time.

Templates can materially reduce repeated setup for migration-heavy suites, but
the saving is workload-specific and must repay template creation and
invalidation. PGSandbox therefore keeps fresh migrations as the correctness
default and the existing template tools as an explicit optimization. This
measurement does not justify making `with-database` template-backed by default.

An adapter that opts into templates should key them by all inputs that affect
the database:

- PostgreSQL major version and selected PGSandbox profile;
- sorted requested extensions and their installed versions;
- a deterministic migration graph or file-content fingerprint;
- schema-affecting settings and seed-data version; and
- an adapter-controlled format version.

Before reuse, compare the key and a schema digest stored with the template.
Treat a mismatch or missing metadata as stale and rebuild from fresh migrations.
Never silently fall back to a stale template. Keep template artifacts local,
create per-run sandboxes from them, apply the normal TTL and cleanup policies,
and let concurrent callers clone independently rather than mutate the template.

## Rowset monolithic versus partitioned

The Rowset suite was measured from commit `935d1762` using its merged
`make test-pgsandbox` adapter, PostgreSQL 18, Redis at `127.0.0.1:6379`, and a
fresh sandbox for every invocation.

The first monolithic run reproduced one failure among 1,173 tests: a blog check
saw Django's missing `frontend/build` warning as an extra error. The sandbox was
still deleted. After the documented consumer prerequisite `npm ci && npm run
build`, the same monolithic command passed all 1,173 tests in 65.39 seconds wall
time (55.41 seconds inside pytest) with 462,962,688 bytes maximum RSS.

Six sequential fresh-sandbox partitions also passed all 1,173 tests:

| Partition | Tests | Wall | Pytest | Max RSS |
| --- | ---: | ---: | ---: | ---: |
| `apps/api` | 91 | 8.80s | 4.33s | 367,149,056 B |
| `apps/core/tests` | 227 | 13.02s | 7.79s | 355,106,816 B |
| `apps/datasets/tests` | 365 | 15.85s | 10.88s | 421,609,472 B |
| `apps/mcp_server/tests` | 106 | 9.53s | 5.84s | 360,939,520 B |
| `apps/pages` | 225 | 11.37s | 7.63s | 386,629,632 B |
| `rowset/tests` + evaluations | 159 | 37.93s | 33.89s | 367,656,960 B |

Sequential partitioning took 96.50 seconds total: about 48% slower than the
passing monolithic run because it paid provisioning, migration, and process
startup six times. Its highest observed RSS was about 9% lower, and failures
would be isolated to a smaller fresh database.

The evidence does not identify a PGSandbox reliability defect. The reproduced
failure was a missing frontend build artifact, and the full suite passed once
that consumer prerequisite existed. Prefer one monolithic session when the
repository is fully prepared and memory is comfortable. Partition by stable
test boundaries for failure isolation, lower peak memory, parallel CI, or when
one process accumulates consumer state; use the structured session result to
compare child, timeout, retention, and cleanup outcomes instead of describing
the mode as generically more reliable.
