---
title: "How to Test PostgreSQL Table Partitioning"
excerpt: "Prove partition topology, boundary routing, row movement, pruning, maintenance operations, exact final state, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-07"
updatedAt: "2026-08-07T06:00:00Z"
tags: ["Postgres", "table partitioning", "integration testing", "query plans", "coding agents"]
category: "Engineering"
metaTitle: "How to Test PostgreSQL Table Partitioning"
metaDescription: "Test PostgreSQL partitioning with topology, boundary routing, row movement, pruning, maintenance, final-state, and cleanup checks."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/test-postgresql-table-partitioning/"
heroImageUrl: ""
featured: false
sortOrder: 158
---
Test PostgreSQL table partitioning with a six-gate proof: declaration, exact boundary routing, row movement, pruning, attach/detach lifecycle, and cleanup. Run a deterministic correctness fixture in a disposable database, benchmark representative data separately, then verify that the sandbox was deleted.

A query returning the right rows does not prove the partition design is correct. The same result can hide an unintended default-partition route, a missing leaf, a boundary error, or a plan that scans every partition. A migration can also route inserts correctly while failing later when an update crosses a boundary or an operator attaches the next partition.

This guide packages those checks into a six-gate **Partition Boundary Proof**: declaration, routing, movement, pruning, lifecycle, and cleanup. PGSandbox MCP supplies a task-scoped database and restricted role on your configured PostgreSQL server. PostgreSQL catalogs, `tableoid`, JSON query plans, exact state assertions, and the deletion result supply the evidence.

*Published and last updated August 7, 2026.*

The complete workflow is:

1. Apply the real migration and inspect the partition key, hierarchy, bounds, and default partition.
2. Insert values on both sides of every boundary and assert their physical tables with `tableoid`.
3. Update partition keys across boundaries and verify the row moved to the intended leaf.
4. Parse `EXPLAIN (FORMAT JSON)` and assert which partitions survive pruning.
5. Exercise attach/detach behavior and verify exact final state.
6. Delete the sandbox and verify the credential-free cleanup result.

## In this guide

- [Understand what partition tests must prove](#what-a-postgresql-partition-test-must-prove)
- [Use the Partition Boundary Proof](#the-partition-boundary-proof)
- [Inspect the installed hierarchy](#1-inspect-the-installed-partition-hierarchy)
- [Run a deterministic harness](#2-run-a-deterministic-partition-routing-test)
- [Test pruning without brittle plans](#3-test-partition-pruning-with-stable-assertions)
- [Test attach and detach maintenance](#4-test-partition-attach-and-detach-boundaries)
- [Run with PGSandbox](#5-run-the-proof-in-a-disposable-database)
- [Record review evidence](#pr-ready-partition-proof)
- [Answer common questions](#postgresql-table-partitioning-testing-faq)

## What a PostgreSQL partition test must prove

PostgreSQL declarative partitioning divides one logical table into child relations, usually local leaf tables but potentially foreign tables. The partitioned parent has no storage of its own; inserts through the parent are routed by the partition key, and an update can move a row when that key crosses a bound. PostgreSQL 18 supports range, list, and hash partitioning, as described in the official [table-partitioning documentation](https://www.postgresql.org/docs/18/ddl-partitioning.html).

Those mechanics create two different proof lanes:

| Proof lane | Question | Stable evidence |
| --- | --- | --- |
| Correctness | Did PostgreSQL install and enforce the intended data boundary? | Catalog definitions, `pg_partition_tree()`, `tableoid`, SQLSTATE, and exact rows |
| Planner behavior | Did this query avoid irrelevant partitions? | Surviving relation names in JSON `EXPLAIN`, plus execution-time loop evidence where needed |

Keep the lanes separate. Partition pruning uses partition bounds, not indexes. An index can improve access inside a surviving partition, but it does not prove that PostgreSQL pruned the other leaves. Conversely, a correctly pruned plan says nothing about whether an exact upper-bound value routed to the next partition or fell into the default.

### Range boundaries are lower-inclusive and upper-exclusive

For a range partition declared `FROM ('2026-01-01') TO ('2026-04-01')`, January 1 belongs to that partition and April 1 does not. The adjacent partition may start at April 1 without overlap. PostgreSQL rejects overlapping bounds, and an insert with no matching leaf fails unless a default partition exists.

That makes boundary fixtures mandatory. For each range, test the lower bound, an interior value close to the upper bound, and the exact upper bound. For discrete types such as `date` or integer, also test the immediate predecessor of the upper bound. For list partitioning, test every declared value, `NULL` when supported by the design, and an unknown value. For hash partitioning, assert the destination PostgreSQL chose rather than reproducing its internal hash in application code.

### The default partition is evidence, not a safety net to ignore

A default partition accepts values not claimed by another leaf. It can prevent uncovered partition-key values from failing solely because no explicit partition matches, but it can also conceal a missing partition indefinitely. Treat an unexpected row in the default partition as a failed routing assertion.

The default also changes maintenance behavior. PostgreSQL may need to scan it before attaching a new explicit partition, because rows already in the default might belong inside the new bounds. The PostgreSQL 18 [`ALTER TABLE` reference](https://www.postgresql.org/docs/18/sql-altertable.html) explains that a suitable exclusionary `CHECK` constraint can avoid that scan. Your maintenance test should cover the conflict before relying on the production runbook.

## The Partition Boundary Proof

A reviewable PostgreSQL partition test should answer six questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Declaration | What hierarchy and keys did the migration install? | `pg_partition_tree()`, `pg_get_partkeydef()`, bounds, default-partition identity |
| Routing | Which physical table owns each edge-case row? | `tableoid::regclass` for lower, inner, upper, gap, and default values |
| Movement | What happens when a partition key changes? | Old/new `tableoid`, leaf counts, missing-destination error where applicable |
| Pruning | Which leaves survive for the real query predicate? | Parsed JSON plan relation names, with a negative control |
| Maintenance | Can the next leaf be attached and an old leaf detached safely? | Validation failure/success, topology change, and standalone detached data |
| Cleanup | Did the destructive proof leave any task database or credential behind? | Credential-free sandbox deletion result and safe database ID |

The useful distinction is that every failure surface remains independently reviewable. Most partitioning examples stop after creating monthly tables and selecting rows. The Partition Boundary Proof separates routing, planner behavior, operational maintenance, and resource cleanup, so one green query cannot hide a broken boundary.

## 1. Inspect the installed partition hierarchy

Apply the repository's real migration first. Recreating similar DDL inside a test proves the example, not the migration under review. The [database migration testing workflow](/blog/database-migration-testing-agent-pr/) covers applying the actual upgrade path before collecting schema and behavior evidence.

Use `pg_partition_tree()` for the hierarchy and `pg_get_partkeydef()` for the parent key:

```sql
SELECT
    relid::regclass::text AS relation,
    parentrelid::regclass::text AS parent,
    isleaf,
    level
FROM pg_partition_tree('public.events'::regclass)
ORDER BY level, relation;

SELECT pg_get_partkeydef('public.events'::regclass);
```

PostgreSQL 18 documents [`pg_partition_tree()`](https://www.postgresql.org/docs/18/functions-admin.html#FUNCTIONS-ADMIN-DBOBJECT) as returning each relation, its immediate parent, whether it is a leaf, and its level. Assert the exact leaf set and parent relationship. A leaf-count-only assertion can pass when one expected partition was replaced by an unintended default or attached under the wrong intermediate parent.

For a simple quarterly range design, the expected result may be:

```text
public.events          parent=NULL          isleaf=false level=0
public.events_2026_q1  parent=public.events isleaf=true  level=1
public.events_2026_q2  parent=public.events isleaf=true  level=1
public.events_default  parent=public.events isleaf=true  level=1
```

Use PostgreSQL's deparser functions rather than parsing internal catalog trees. `pg_class.relpartbound` is stored as an internal `pg_node_tree`; [`pg_get_partition_constraintdef()` and related system-information functions](https://www.postgresql.org/docs/18/functions-info.html) provide human-readable definitions. Normalize whitespace before comparing decompiled output, because it is not guaranteed to reproduce the original DDL byte for byte.

Also verify constraints that span partitions. A `PRIMARY KEY` or `UNIQUE` constraint declared on a partitioned table must include every partition-key column, and such a constraint cannot be declared when the partition key contains expressions. The physical child indexes cannot enforce uniqueness across unrelated leaves by themselves. That limitation belongs in migration tests, especially when an ORM generated the DDL.

PGSandbox `describe_schema` identifies the parent as a `partitioned_table`, and schema digests count partitioned relations. Those are useful coarse migration checks. They do not preserve the exact strategy, key, default identity, or child bounds, so use PostgreSQL catalogs for this declaration gate.

## 2. Run a deterministic partition-routing test

The following Psycopg harness creates a small quarterly fixture, proves topology and edge routing, then moves a row across a boundary. In an application repository, replace `install_fixture()` with the real migration command and keep the catalog and behavior assertions against the migrated tables.

```python
import json
import os

import psycopg


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]


def connect():
    return psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    )


def install_fixture(conn):
    conn.execute("DROP TABLE IF EXISTS events CASCADE")
    conn.execute(
        """
        CREATE TABLE events (
            event_id bigint NOT NULL,
            occurred_on date NOT NULL,
            payload text NOT NULL,
            PRIMARY KEY (event_id, occurred_on)
        ) PARTITION BY RANGE (occurred_on)
        """
    )
    conn.execute(
        """
        CREATE TABLE events_2026_q1 PARTITION OF events
        FOR VALUES FROM ('2026-01-01') TO ('2026-04-01')
        """
    )
    conn.execute(
        """
        CREATE TABLE events_2026_q2 PARTITION OF events
        FOR VALUES FROM ('2026-04-01') TO ('2026-07-01')
        """
    )
    conn.execute(
        "CREATE TABLE events_default PARTITION OF events DEFAULT"
    )


def topology(conn):
    return conn.execute(
        """
        SELECT
            relid::regclass::text,
            COALESCE(parentrelid::regclass::text, ''),
            isleaf,
            level
        FROM pg_partition_tree('events'::regclass)
        ORDER BY level, relid::regclass::text
        """
    ).fetchall()


def route_map(conn):
    return dict(
        conn.execute(
            """
            SELECT event_id, tableoid::regclass::text
            FROM events
            ORDER BY event_id
            """
        ).fetchall()
    )


def planned_relations(conn, start_date, end_date):
    raw = conn.execute(
        """
        EXPLAIN (FORMAT JSON, COSTS FALSE)
        SELECT event_id
        FROM events
        WHERE occurred_on >= %s AND occurred_on < %s
        """,
        (start_date, end_date),
    ).fetchone()[0]
    plan = raw if isinstance(raw, list) else json.loads(raw)

    relations = set()

    def walk(node):
        if "Relation Name" in node:
            relations.add(node["Relation Name"])
        for child in node.get("Plans", []):
            walk(child)

    walk(plan[0]["Plan"])
    return relations


def declaration(conn):
    strategy, key, default_name = conn.execute(
        """
        SELECT
            partstrat,
            pg_get_partkeydef(partrelid),
            NULLIF(partdefid, 0)::regclass::text
        FROM pg_partitioned_table
        WHERE partrelid = 'events'::regclass
        """
    ).fetchone()

    bounds = dict(
        conn.execute(
            """
            SELECT
                child.relname,
                pg_get_expr(child.relpartbound, child.oid)
            FROM pg_inherits AS inheritance
            JOIN pg_class AS child
              ON child.oid = inheritance.inhrelid
            WHERE inheritance.inhparent = 'events'::regclass
            ORDER BY child.relname
            """
        ).fetchall()
    )
    return strategy, key, default_name, bounds


with connect() as conn:
    install_fixture(conn)

    strategy, key, default_name, bounds = declaration(conn)
    assert strategy == "r"
    assert "RANGE" in key and "occurred_on" in key
    assert default_name == "events_default"
    assert "2026-01-01" in bounds["events_2026_q1"]
    assert "2026-04-01" in bounds["events_2026_q1"]
    assert "2026-04-01" in bounds["events_2026_q2"]
    assert "2026-07-01" in bounds["events_2026_q2"]
    assert bounds["events_default"] == "DEFAULT"

    leaves = {
        row[0]
        for row in topology(conn)
        if row[2]
    }
    assert leaves == {
        "events_2026_q1",
        "events_2026_q2",
        "events_default",
    }

    with conn.cursor() as cursor:
        cursor.executemany(
            """
            INSERT INTO events (event_id, occurred_on, payload)
            VALUES (%s, %s, %s)
            """,
            [
                (0, "2025-12-31", "before first explicit range"),
                (1, "2026-01-01", "q1 lower bound"),
                (2, "2026-03-31", "q1 upper edge"),
                (3, "2026-04-01", "q2 lower bound"),
                (4, "2026-06-30", "q2 upper edge"),
                (5, "2026-07-01", "unclaimed future date"),
            ],
        )

    assert route_map(conn) == {
        0: "events_default",
        1: "events_2026_q1",
        2: "events_2026_q1",
        3: "events_2026_q2",
        4: "events_2026_q2",
        5: "events_default",
    }

    before_count = conn.execute(
        "SELECT count(*) FROM events"
    ).fetchone()[0]
    conn.execute(
        """
        UPDATE events
        SET occurred_on = DATE '2026-04-02',
            payload = 'moved from q1 to q2'
        WHERE event_id = 2
          AND occurred_on = DATE '2026-03-31'
        """
    )
    moved = conn.execute(
        """
        SELECT payload, tableoid::regclass::text
        FROM events
        WHERE event_id = 2
        """
    ).fetchone()
    assert moved == ("moved from q1 to q2", "events_2026_q2")
    assert conn.execute(
        "SELECT count(*) FROM events_2026_q1 WHERE event_id = 2"
    ).fetchone()[0] == 0
    assert conn.execute(
        "SELECT count(*) FROM events"
    ).fetchone()[0] == before_count

    assert planned_relations(
        conn, "2026-01-01", "2026-04-01"
    ) == {"events_2026_q1"}
    assert planned_relations(
        conn, "2026-01-01", "2026-07-01"
    ) == {"events_2026_q1", "events_2026_q2"}

    conn.execute("SET enable_partition_pruning = off")
    try:
        assert planned_relations(
            conn, "2026-01-01", "2026-04-01"
        ) == {
            "events_2026_q1",
            "events_2026_q2",
            "events_default",
        }
    finally:
        conn.execute("RESET enable_partition_pruning")

    conn.execute("DELETE FROM events WHERE event_id IN (0, 5)")

    final_rows = conn.execute(
        """
        SELECT event_id, occurred_on::text, tableoid::regclass::text
        FROM events
        ORDER BY event_id
        """
    ).fetchall()
    assert final_rows == [
        (1, "2026-01-01", "events_2026_q1"),
        (2, "2026-04-02", "events_2026_q2"),
        (3, "2026-04-01", "events_2026_q2"),
        (4, "2026-06-30", "events_2026_q2"),
    ]
```

The `tableoid` system column is the direct routing proof. PostgreSQL's [system-column documentation](https://www.postgresql.org/docs/18/ddl-system-columns.html) defines it as the OID of the table containing the row; casting it to `regclass` exposes the physical partition name. Selecting only business columns through the parent cannot tell you which leaf stored the row.

*Verification note:* We executed the correctness, pruning-control, movement, attach, and detach fixture on August 7, 2026 against a PGSandbox-created disposable database on a configured PostgreSQL 15 profile with Psycopg 3.3.4; the sandbox was deleted after the run. The version-sensitive guidance is grounded in fixed PostgreSQL 18 documentation. Re-run the fixture on every PostgreSQL major your application supports.

Add a second negative lane when the production design has no default partition. Insert an uncovered value and assert PostgreSQL rejects it. When the design does have a default partition, assert the default route explicitly and decide whether any such row is allowed by the application contract.

Capture the stable SQLSTATE rather than PostgreSQL's message text:

```python
conn.execute("DROP TABLE IF EXISTS no_default_events CASCADE")
conn.execute(
    """
    CREATE TABLE no_default_events (occurred_on date NOT NULL)
    PARTITION BY RANGE (occurred_on)
    """
)
conn.execute(
    """
    CREATE TABLE no_default_events_q1
    PARTITION OF no_default_events
    FOR VALUES FROM ('2026-01-01') TO ('2026-04-01')
    """
)
try:
    conn.execute(
        "INSERT INTO no_default_events VALUES (DATE '2026-07-01')"
    )
    raise AssertionError("expected uncovered partition key to fail")
except psycopg.Error as exc:
    assert exc.sqlstate == "23514"
```

## 3. Test partition pruning with stable assertions

PostgreSQL can prune partitions during planning, at executor initialization, or while a parameterized plan runs. The [partition-pruning documentation](https://www.postgresql.org/docs/18/ddl-partitioning.html#DDL-PARTITION-PRUNING) notes that initialization pruning appears as `Subplans Removed`, while execution-time pruning may be visible through loop counts or `(never executed)` in text plans.

For a normal constant range predicate, parse `EXPLAIN (FORMAT JSON)` and assert the surviving leaf names. Do not snapshot the whole plan. Costs, scan nodes, estimates, and timings change with fixture size, `ANALYZE`, PostgreSQL releases, and host settings.

Use two controls:

1. A narrow predicate that should leave one partition.
2. A broad predicate that should retain two or more partitions.

You can also compare a session with `enable_partition_pruning = on` against one with it disabled. The setting defaults to on, but toggling it makes the proof diagnostic: if both plans visit the same leaves, either the predicate cannot imply the bounds or the assertion is reading the plan incorrectly.

`EXPLAIN ANALYZE` executes the statement. The PostgreSQL 18 [`EXPLAIN` reference](https://www.postgresql.org/docs/18/sql-explain.html) recommends wrapping data-changing statements in `BEGIN` and `ROLLBACK` when you need execution evidence without retaining changes. For read-only pruning checks, JSON `EXPLAIN` is usually enough and avoids measurement overhead.

Prepared statements deserve their own case when the application uses them. A generic parameterized plan may prune at execution rather than planning time. Assert executed relation names or loop counts for the supplied parameter; do not fail only because every potential subplan appears in the static tree.

For execution-time pruning, run `EXPLAIN (ANALYZE, FORMAT JSON) EXECUTE ...` for a read-only prepared statement and count a child as executed only when `Actual Loops` is greater than zero. A relation name appearing in the generic plan is not evidence that PostgreSQL scanned it.

### Validate statistics and performance separately

The deterministic fixture above proves correctness; five rows are not representative performance data. To decide whether partitioning helps the application workload, compare partitioned and unpartitioned tables with identical columns, indexes, data distribution, and row counts on the same PostgreSQL build and hardware.

Run `ANALYZE` after loading both variants. PostgreSQL's [routine vacuuming documentation](https://www.postgresql.org/docs/18/routine-vacuuming.html#VACUUM-FOR-STATISTICS) notes that autovacuum does not process partitioned parents, so parent statistics may need a manual `ANALYZE` when queries span partitions. Inspect `pg_stats` for the parent and relevant leaves before interpreting estimates.

Then run the real query mix repeatedly under documented cache conditions. Capture planning time, execution time, buffers, rows, transactions per second, and latency percentiles. PostgreSQL 18 [`pgbench`](https://www.postgresql.org/docs/18/pgbench.html) can initialize partitioned benchmark tables with `--partitions` and `--partition-method`, but its built-in workload is only a starting point. The decision should follow the application's predicates and maintenance pattern, not a borrowed headline number.

Use the benchmark as a separate evidence packet:

```text
partition benchmark
- PostgreSQL/build and host: recorded
- schema/indexes/data distribution: identical
- ANALYZE state: parent and leaves recorded
- query mix and concurrency: application-representative
- warmup and repeated runs: recorded
- output: planning/execution time, buffers, TPS, p50/p95/p99 latency
```

A partition design passes the correctness proof before it enters this benchmark. Performance can improve for one query family and regress for another, so report both rather than collapsing the result into “partitioning is faster.”

## 4. Test partition attach and detach boundaries

Partition maintenance is part of the data contract. A quarterly design that cannot attach the next quarter safely will eventually fail even if every current query is correct.

Test this sequence in a disposable database:

1. Create a standalone table with `LIKE parent INCLUDING ALL` and add a `CHECK` constraint matching the proposed bound.
2. Load one valid row into the standalone table.
3. Insert a conflicting row for the same future bound through the parent so it lands in the default partition.
4. Attempt `ATTACH PARTITION` and assert the default-partition conflict fails with SQLSTATE `23514`.
5. Remove the conflicting default row, attach the table, and verify routing through the parent reaches the new leaf.
6. Detach it, prove the standalone table retains its row, and verify new values for that bound return to the default partition.

The following continuation uses the earlier `events` fixture after its July default row has been removed:

```python
conn.execute("DROP TABLE IF EXISTS events_2026_q3")
conn.execute("CREATE TABLE events_2026_q3 (LIKE events INCLUDING ALL)")
conn.execute(
    """
    ALTER TABLE events_2026_q3
    ADD CONSTRAINT events_2026_q3_bound
    CHECK (
        occurred_on >= DATE '2026-07-01'
        AND occurred_on < DATE '2026-10-01'
    )
    """
)
conn.execute(
    """
    INSERT INTO events_2026_q3 VALUES
        (6, DATE '2026-07-15', 'preloaded q3 row')
    """
)
conn.execute(
    """
    INSERT INTO events VALUES
        (7, DATE '2026-07-20', 'conflicting default row')
    """
)

try:
    conn.execute(
        """
        ALTER TABLE events ATTACH PARTITION events_2026_q3
        FOR VALUES FROM ('2026-07-01') TO ('2026-10-01')
        """
    )
    raise AssertionError("expected default-partition conflict")
except psycopg.Error as exc:
    assert exc.sqlstate == "23514"

conn.execute("DELETE FROM events WHERE event_id = 7")
conn.execute(
    """
    ALTER TABLE events ATTACH PARTITION events_2026_q3
    FOR VALUES FROM ('2026-07-01') TO ('2026-10-01')
    """
)
assert route_map(conn)[6] == "events_2026_q3"

conn.execute("ALTER TABLE events DETACH PARTITION events_2026_q3")
assert conn.execute(
    "SELECT count(*) FROM events WHERE event_id = 6"
).fetchone()[0] == 0
assert conn.execute(
    "SELECT count(*) FROM events_2026_q3 WHERE event_id = 6"
).fetchone()[0] == 1

conn.execute(
    "INSERT INTO events VALUES (8, DATE '2026-07-25', 'default again')"
)
assert route_map(conn)[8] == "events_default"
```

Attaching a preloaded table normally requires PostgreSQL to validate its contents. For a simple column-based range partition, a valid matching `CHECK` can let PostgreSQL skip the attached-table scan. Expression keys and nullable list keys have additional requirements. A default partition must first be cleared of rows belonging to the new bound before its exclusionary check can validate.

`DETACH PARTITION` preserves the detached table. `DETACH PARTITION CONCURRENTLY` reduces the parent lock level but has important restrictions: it cannot run inside a transaction block, it is unavailable when the parent has a default partition, and an interrupted detach may require `FINALIZE`. Exercise the exact maintenance form used by the production runbook instead of assuming the concurrent form is interchangeable.

For index maintenance, remember that an index declared on the parent is virtual and backed by child indexes. PostgreSQL does not allow `CREATE INDEX CONCURRENTLY` directly on the partitioned parent. If the production migration uses the documented `ON ONLY` parent plus per-leaf concurrent indexes and `ATTACH PARTITION` index workflow, test the complete sequence separately; a small routing harness does not prove online index rollout behavior.

## 5. Run the proof in a disposable database

Do not run a destructive partition harness against a shared development database. It drops fixture tables, moves rows, changes topology, and may intentionally trigger validation errors. Create one task-scoped database for the proof.

With PGSandbox MCP, the workflow is:

1. Call `create_database` with a short TTL, owner, and purpose.
2. Obtain the sandbox connection through `get_connection_string` and inject it as `PGSANDBOX_DATABASE_URL` without logging the value.
3. Apply the repository's migration and run the partition harness.
4. Collect bounded schema, routing, plan, and final-state evidence.
5. Call `delete_database` in a `finally` path; retain a failed sandbox only when a human needs the TTL-bounded debugging window.

The [MCP tool contract](/docs/mcp-tools/) separates lifecycle authority from SQL executed by the scoped sandbox role. For a repository test command, the [disposable integration-test workflow](/blog/run-integration-tests-disposable-postgres-database/) shows the one-shot alternative:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint "partition proof" \
  --owner "agent-ci" \
  --ttl-minutes 45 \
  --cleanup always \
  -- uv run --with psycopg python tests/test_partitioning.py
```

The child process receives a database URL while PGSandbox returns a credential-free result envelope. Keep the URL out of logs, screenshots, PR descriptions, and test artifacts. Report the safe database ID, selected PostgreSQL version, test status, and cleanup outcome instead.

For agent-authored migrations, pair the behavior test with a [schema snapshot and compact diff](/blog/postgres-schema-snapshots-agent-migration-reviews/). The snapshot proves which partitioned objects changed; the harness proves their routing and planner behavior.

## PR-ready partition proof

A reviewer should be able to evaluate the migration without opening raw database logs. Keep the packet small and structured:

```text
PostgreSQL partition proof
- server: PostgreSQL 18.x / disposable sandbox
- topology: RANGE (occurred_on); q1, q2, default leaves verified
- routing: lower/inner/upper/default edge cases verified with tableoid
- movement: event 2 moved q1 -> q2 after partition-key update
- pruning: q1 predicate retained only events_2026_q1
- maintenance: invalid attach rejected; valid attach/detach path verified
- final state: 4 expected rows, exact physical leaves verified
- cleanup: sandbox deleted
```

Keep full JSON plans in CI artifacts only when a failure needs diagnosis. The review summary should record stable facts: installed key, leaf set, routed tables, executed relations, SQLSTATE where relevant, exact final rows, and cleanup status.

The [bounded SQL workflow](/blog/postgres-run-sql-bounded-results/) is useful for compact catalog and row evidence. If a query-plan regression is the focus, use the companion [PostgreSQL EXPLAIN review guide](/blog/postgres-explain-plan-agent-sql/) to distinguish plan eligibility from brittle performance expectations.

## Common partition-testing mistakes

### Asserting only returned values

A parent-table query can return the expected rows even when they live in the default or wrong leaf. Include `tableoid::regclass` in routing assertions.

### Testing one value per partition

One middle value misses inclusive/exclusive mistakes. Test exact lower and upper boundaries, gaps, `NULL` rules where relevant, and an uncovered/default value.

### Snapshotting an entire EXPLAIN plan

Full plans change for legitimate reasons. Parse JSON and assert stable semantics such as surviving relation names, `Subplans Removed`, or execution loops. Test inner-partition indexes separately.

### Ignoring partition-key updates

An insert-only test misses row movement. PostgreSQL implements a cross-partition update as a delete and insert internally, and concurrent changes can expose different failure behavior. Include at least one move and one missing-destination control.

### Skipping the next-partition operation

Routing today is not enough. Attach and detach are recurring operational paths for time-based partitioning. Test validation, default-partition conflicts, topology changes, and retained standalone data.

### Benchmarking before proving correctness

Partitioning is useful when the workload and table shape justify it, but a faster test run cannot compensate for a boundary defect. Prove topology, routing, and pruning first. Benchmark representative data and query patterns separately, with repeated runs and environment details.

## PostgreSQL table partitioning testing FAQ

### How do I verify which PostgreSQL partition contains a row?

Select `tableoid::regclass` with the row. `tableoid` identifies the physical table that stores it, while the `regclass` cast renders the relation name. Assert that name for every boundary fixture instead of inferring the destination from returned business columns.

### How do I test PostgreSQL partition pruning?

Run `EXPLAIN (FORMAT JSON)` for the application's real predicate and recursively collect `Relation Name` values. Assert the expected leaf set, then add a broader negative control. For parameterized plans, inspect execution-time loop evidence because pruning may happen after planning.

### Should a partition test use a default partition?

Match production. If production has a default, assert which values reach it and treat unexpected rows as failures. Also test the default-partition constraint before attaching a new explicit range. If production has no default, assert uncovered inserts fail.

### What should I test at a range-partition boundary?

Test the exact lower bound, an interior value close to the upper bound, and the exact upper bound. For discrete types such as `date` or integer, also test the immediate predecessor of the upper bound. The upper-bound value must route to the adjacent partition, the default, or an error.

### Does partition pruning require an index?

No. PostgreSQL prunes from partition bounds, not indexes. Indexes may improve access inside the leaves that survive pruning. Test the surviving partition set and the per-leaf access path as separate claims.

### How many PostgreSQL partitions are too many?

There is no universal limit. PostgreSQL 18 notes that a few thousand can work when typical queries prune to a small subset, but planning time and per-session memory grow with the remaining partition set. Benchmark the real workload and report the environment instead of adopting a fixed threshold.

### How do I know whether PostgreSQL partitioning improves my workload?

Benchmark partitioned and unpartitioned variants with identical schema, indexes, data, host, and query mix. Run `ANALYZE`, repeat the workload under documented cache conditions, and compare planning time, execution time, buffers, throughput, and latency percentiles. Report regressions as well as wins.

### Why is PostgreSQL scanning every partition?

The predicate may not imply the partition bounds, pruning may be disabled, or a parameterized plan may defer pruning until execution. Inspect JSON `EXPLAIN`, verify `enable_partition_pruning`, and use `EXPLAIN ANALYZE` loop counts for execution-time pruning before blaming indexes.

### Does autovacuum analyze a partitioned parent table?

No. PostgreSQL 18 documents that autovacuum does not process partitioned parent tables, although it processes ordinary leaf partitions. Run manual `ANALYZE` when parent statistics matter for multi-partition planning, then inspect `pg_stats` before interpreting estimates.

### How should I test prepared statements for partition pruning?

Execute the prepared statement with `EXPLAIN (ANALYZE, FORMAT JSON)` against representative parameters. Count a child as executed only when `Actual Loops` is greater than zero. A child merely appearing in a generic plan may still be pruned during execution.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "BlogPosting",
      "headline": "How to Test PostgreSQL Table Partitioning",
      "description": "Test PostgreSQL partitioning with topology, boundary routing, row movement, pruning, maintenance, final-state, and cleanup checks.",
      "datePublished": "2026-08-07",
      "dateModified": "2026-08-07",
      "author": {"@type": "Organization", "name": "PGSandbox Team"},
      "publisher": {"@type": "Organization", "name": "PGSandbox MCP"},
      "mainEntityOfPage": "https://pgsandbox.lvtd.dev/blog/test-postgresql-table-partitioning/"
    },
    {
      "@type": "HowTo",
      "name": "How to test PostgreSQL table partitioning",
      "description": "Prove partition topology, boundary routing, row movement, pruning, maintenance operations, exact final state, and disposable cleanup.",
      "step": [
        {"@type": "HowToStep", "name": "Inspect the hierarchy", "text": "Apply the real migration and verify the partition key, bounds, hierarchy, and default leaf."},
        {"@type": "HowToStep", "name": "Prove boundary routing", "text": "Insert lower, inner, upper, gap, and default values and assert their physical tables with tableoid."},
        {"@type": "HowToStep", "name": "Prove row movement", "text": "Update a partition key across a bound and verify the row moved to the intended leaf."},
        {"@type": "HowToStep", "name": "Verify pruning", "text": "Parse a JSON EXPLAIN plan and assert which partition relations survive for the real query predicate."},
        {"@type": "HowToStep", "name": "Exercise maintenance", "text": "Test invalid and valid attach behavior, detach the leaf, and verify exact final state."},
        {"@type": "HowToStep", "name": "Verify cleanup", "text": "Delete the disposable database and record the credential-free cleanup result."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do I verify which PostgreSQL partition contains a row?", "acceptedAnswer": {"@type": "Answer", "text": "Select tableoid::regclass with the row. tableoid identifies the physical table that stores it, while the regclass cast renders the relation name. Assert that name for every boundary fixture."}},
        {"@type": "Question", "name": "How do I test PostgreSQL partition pruning?", "acceptedAnswer": {"@type": "Answer", "text": "Run EXPLAIN (FORMAT JSON) for the real predicate, recursively collect Relation Name values, and assert the expected leaf set. Inspect execution loops when a parameterized plan prunes during execution."}},
        {"@type": "Question", "name": "Should a partition test use a default partition?", "acceptedAnswer": {"@type": "Answer", "text": "Match production. If a default exists, assert expected routes and test its constraint before attaching a new range. Without a default, assert that uncovered inserts fail."}},
        {"@type": "Question", "name": "What should I test at a range-partition boundary?", "acceptedAnswer": {"@type": "Answer", "text": "Test the exact lower bound, an interior value close to the upper bound, and the exact upper bound. For discrete types, also test the immediate predecessor. Lower bounds are inclusive and upper bounds are exclusive."}},
        {"@type": "Question", "name": "Does partition pruning require an index?", "acceptedAnswer": {"@type": "Answer", "text": "No. PostgreSQL prunes from partition bounds, not indexes. Test the surviving partition set separately from the access path inside each surviving leaf."}},
        {"@type": "Question", "name": "How many PostgreSQL partitions are too many?", "acceptedAnswer": {"@type": "Answer", "text": "There is no universal limit. Planning time and per-session memory depend on how many partitions remain after pruning, the workload, PostgreSQL version, and hardware. Benchmark the real workload instead of adopting a fixed threshold."}},
        {"@type": "Question", "name": "How do I know whether PostgreSQL partitioning improves my workload?", "acceptedAnswer": {"@type": "Answer", "text": "Benchmark partitioned and unpartitioned variants with identical schema, indexes, data, host, and query mix. Compare planning time, execution time, buffers, throughput, and latency percentiles across repeated runs."}},
        {"@type": "Question", "name": "Why is PostgreSQL scanning every partition?", "acceptedAnswer": {"@type": "Answer", "text": "The predicate may not imply the bounds, pruning may be disabled, or a parameterized plan may defer pruning until execution. Verify the pruning setting and inspect JSON plan and execution-loop evidence."}},
        {"@type": "Question", "name": "Does autovacuum analyze a partitioned parent table?", "acceptedAnswer": {"@type": "Answer", "text": "No. Autovacuum processes ordinary leaf partitions but not the partitioned parent. Run manual ANALYZE when parent statistics matter for multi-partition planning."}},
        {"@type": "Question", "name": "How should I test prepared statements for partition pruning?", "acceptedAnswer": {"@type": "Answer", "text": "Use EXPLAIN (ANALYZE, FORMAT JSON) with representative parameters and count a child as executed only when Actual Loops is greater than zero."}}
      ]
    }
  ]
}
</script>
