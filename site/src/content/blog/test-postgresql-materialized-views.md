---
title: "How to Test PostgreSQL Materialized Views"
excerpt: "Prove the installed definition, stale-data boundary, refresh behavior, concurrent-refresh prerequisites, exact results, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-10"
updatedAt: "2026-08-10T06:00:00Z"
tags: ["Postgres", "materialized views", "integration testing", "database migrations", "coding agents"]
category: "Engineering"
metaTitle: "How to Test PostgreSQL Materialized Views"
metaDescription: "Test PostgreSQL materialized views with definition, staleness, refresh, concurrency, exact-result, and disposable-cleanup checks."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/test-postgresql-materialized-views/"
heroImageUrl: ""
featured: false
sortOrder: 159
---
Test PostgreSQL materialized views with six independent checks: installed definition, initial population, deliberate staleness, ordinary refresh, concurrent-refresh readiness, and cleanup. Apply the real migration in a disposable database, mutate the source tables, and assert exactly when the stored result does and does not change.

A passing `SELECT` from a materialized view proves very little by itself. The rows may have been populated by the migration and never refreshed again. A test can also miss a broken `CONCURRENTLY` path, an unsuitable unique index, or a deployment that created the right relation from the wrong query.

This guide packages the checks into a **Materialized View Refresh Proof**. PGSandbox MCP supplies a task-scoped database and restricted role on your configured PostgreSQL server. PostgreSQL catalogs, deterministic source rows, explicit refresh commands, exact result assertions, and sandbox deletion supply the evidence.

*Published and last updated August 10, 2026.*

The workflow is:

1. Apply the real migration and inspect `pg_matviews` plus the installed indexes.
2. Assert the materialized view's initial rows.
3. Change a source table and prove the stored result stays stale before refresh.
4. Run an ordinary refresh and assert the new exact result.
5. Prove the concurrent-refresh preconditions and then run `REFRESH MATERIALIZED VIEW CONCURRENTLY`.
6. Record final state and delete the disposable database.

## In this guide

- [Understand the materialized-view boundary](#what-a-postgresql-materialized-view-test-must-prove)
- [Use the Materialized View Refresh Proof](#the-materialized-view-refresh-proof)
- [Inspect the installed definition](#1-inspect-the-installed-materialized-view)
- [Run the deterministic harness](#2-run-a-deterministic-materialized-view-test)
- [Test concurrent refresh correctly](#3-test-concurrent-refresh-without-false-confidence)
- [Run the proof with PGSandbox](#4-run-the-proof-in-a-disposable-database)
- [Record review evidence](#pr-ready-materialized-view-proof)
- [Answer common questions](#postgresql-materialized-view-testing-faq)

## What a PostgreSQL materialized view test must prove

A PostgreSQL materialized view stores the result of a query in a table-like relation. Reads use those stored rows; PostgreSQL does not run the defining query again for every `SELECT`. The official [materialized-view documentation](https://www.postgresql.org/docs/current/rules-materializedviews.html) explains the central tradeoff: reads can be faster, but the stored data is not necessarily current.

That tradeoff creates two separate contracts:

| Contract | Question | Stable evidence |
| --- | --- | --- |
| Definition | Did the migration install the intended query and indexes? | `pg_matviews.definition`, `ispopulated`, owner, tablespace, and current `pg_index` rows |
| Refresh | Do source changes become visible only through the intended refresh path? | Exact rows before mutation, while stale, after ordinary refresh, and after concurrent refresh |

Do not collapse them into one assertion. A correct definition can still have a broken scheduler or missing unique index. A refresh can return plausible totals even when the migration installed an unintended filter.

### Materialized views are snapshots, not live views

After source rows change, the materialized view keeps its previous stored result until `REFRESH MATERIALIZED VIEW` replaces it. This stale interval is expected behavior, so it belongs in the test rather than being treated as an accident.

The [PostgreSQL `REFRESH MATERIALIZED VIEW` reference](https://www.postgresql.org/docs/current/sql-refreshmaterializedview.html) also documents two useful boundaries:

- `WITH NO DATA` leaves the materialized view unscannable until a later refresh with data.
- `CONCURRENTLY` and `WITH NO DATA` cannot be used together.

A useful test proves both a positive state and a negative boundary. Insert a source row, assert that the materialized result remains unchanged, refresh, then assert the new result. That sequence distinguishes stored snapshot behavior from an ordinary view.

### Concurrent refresh has structural prerequisites

`REFRESH MATERIALIZED VIEW CONCURRENTLY` keeps concurrent readers from being locked out, but PostgreSQL requires at least one qualifying `UNIQUE` index on the materialized view. The index must use only column names, include every row, and cannot be a partial or expression index. The materialized view must already be populated. PostgreSQL also permits only one refresh at a time for a given materialized view.

Those are migration properties, not deployment trivia. If production uses concurrent refresh, the migration test should inspect the unique index and execute the concurrent command. Merely checking that *an* index exists is too weak.

## The Materialized View Refresh Proof

A reviewable materialized-view test should answer six questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Declaration | What query and storage state did the migration install? | Normalized `pg_matviews.definition`, owner, tablespace, and `ispopulated` |
| Initial population | What exact rows were stored at creation time? | Ordered rows or a deterministic digest |
| Staleness | Does a source-table mutation leave the stored result unchanged before refresh? | Source count plus unchanged materialized rows |
| Ordinary refresh | Does a blocking refresh replace the result correctly? | Exact rows after `REFRESH MATERIALIZED VIEW` |
| Concurrent readiness | Can the production refresh mode run with a qualifying index and populated view? | Unique-index catalog evidence plus successful concurrent refresh |
| Cleanup | Did the test remove its database and role? | Credential-free sandbox deletion result |

This is the article's information-gain point: treat **staleness as a state to prove**, not a wait to tolerate. The result sequence `initial -> source changed/view stale -> ordinary refresh -> source changed/view stale -> concurrent refresh` turns refresh semantics into deterministic review evidence.

## 1. Inspect the installed materialized view

Apply the repository's real migration first. Recreating similar DDL inside the test proves the example, not the migration under review. The [database migration testing guide](/blog/database-migration-testing-agent-pr/) covers that command-and-evidence boundary.

PostgreSQL exposes materialized-view metadata through [`pg_matviews`](https://www.postgresql.org/docs/current/view-pg-matviews.html):

```sql
SELECT
    schemaname,
    matviewname,
    matviewowner,
    tablespace,
    hasindexes,
    ispopulated,
    definition
FROM pg_matviews
WHERE schemaname = 'public'
  AND matviewname = 'account_totals';
```

Normalize whitespace before comparing the deparsed `definition`; do not require a byte-for-byte copy of the migration text. Assert the meaningful parts: source relations, selected columns, joins, filters, grouping keys, and aggregate expressions.

Inspect the indexes separately:

```sql
SELECT indexname, indexdef
FROM pg_indexes
WHERE schemaname = 'public'
  AND tablename = 'account_totals'
ORDER BY indexname;
```

For concurrent refresh, assert the exact qualifying unique key. A partial unique index such as `WHERE total_cents > 0` does not satisfy PostgreSQL's requirement because it does not include all rows. An expression index such as `UNIQUE (lower(account_code))` does not satisfy it either.

A [schema snapshot](/blog/postgres-schema-snapshots-agent-migration-reviews/) can show that the materialized view and index appeared. Keep the behavioral refresh proof as a separate artifact because a schema diff cannot show whether stored rows become stale and fresh at the intended times.

## 2. Run a deterministic materialized-view test

The following Psycopg harness installs a small fixture, proves the stale-data boundary, exercises both refresh modes, and checks exact rows. In an application repository, replace `install_fixture()` with the real migration command and retain the assertions against the migrated objects.

```python
import os
import re

import psycopg


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]


def rows(conn):
    return conn.execute(
        """
        SELECT account_id, order_count, total_cents
        FROM account_totals
        ORDER BY account_id
        """
    ).fetchall()


def install_fixture(conn):
    conn.execute("DROP MATERIALIZED VIEW IF EXISTS account_totals")
    conn.execute("DROP TABLE IF EXISTS orders")
    conn.execute(
        """
        CREATE TABLE orders (
            order_id bigint PRIMARY KEY,
            account_id bigint NOT NULL,
            amount_cents bigint NOT NULL CHECK (amount_cents >= 0),
            status text NOT NULL
        )
        """
    )
    conn.execute(
        """
        INSERT INTO orders
            (order_id, account_id, amount_cents, status)
        VALUES
            (1, 10, 1200, 'paid'),
            (2, 10, 800, 'paid'),
            (3, 20, 5000, 'pending')
        """
    )
    conn.execute(
        """
        CREATE MATERIALIZED VIEW account_totals AS
        SELECT
            account_id,
            count(*)::bigint AS order_count,
            sum(amount_cents)::bigint AS total_cents
        FROM orders
        WHERE status = 'paid'
        GROUP BY account_id
        """
    )


with psycopg.connect(DATABASE_URL, autocommit=True) as conn:
    install_fixture(conn)

    definition, is_populated = conn.execute(
        """
        SELECT definition, ispopulated
        FROM pg_matviews
        WHERE schemaname = 'public'
          AND matviewname = 'account_totals'
        """
    ).fetchone()
    normalized = re.sub(r"\s+", " ", definition).lower()
    assert "from orders" in normalized
    assert "status = 'paid'" in normalized
    assert re.search(r"group by (?:orders\.)?account_id", normalized)
    assert is_populated is True
    assert rows(conn) == [(10, 2, 2000)]

    # Change the source. The materialized result must remain stale.
    conn.execute(
        """
        INSERT INTO orders
            (order_id, account_id, amount_cents, status)
        VALUES
            (4, 10, 300, 'paid'),
            (5, 20, 700, 'paid')
        """
    )
    assert conn.execute("SELECT count(*) FROM orders").fetchone()[0] == 5
    assert rows(conn) == [(10, 2, 2000)]

    # Ordinary refresh replaces the stored result.
    conn.execute("REFRESH MATERIALIZED VIEW account_totals")
    assert rows(conn) == [(10, 3, 2300), (20, 1, 700)]

    # CONCURRENTLY must fail before a qualifying unique index exists.
    try:
        conn.execute(
            "REFRESH MATERIALIZED VIEW CONCURRENTLY account_totals"
        )
        raise AssertionError("concurrent refresh unexpectedly succeeded")
    except psycopg.errors.ObjectNotInPrerequisiteState as exc:
        assert exc.sqlstate == "55000"

    # Install the all-rows, column-only unique index required by CONCURRENTLY.
    conn.execute(
        "CREATE UNIQUE INDEX account_totals_account_id_key "
        "ON account_totals (account_id)"
    )
    eligibility = conn.execute(
        """
        SELECT
            i.indisunique,
            i.indisvalid,
            i.indexprs IS NULL,
            i.indpred IS NULL,
            (
                SELECT array_agg(a.attname ORDER BY key.ordinality)
                FROM unnest(i.indkey) WITH ORDINALITY
                    AS key(attnum, ordinality)
                JOIN pg_attribute AS a
                  ON a.attrelid = i.indrelid
                 AND a.attnum = key.attnum
            )
        FROM pg_index AS i
        WHERE i.indexrelid =
            'account_totals_account_id_key'::regclass
        """
    ).fetchone()
    assert eligibility == (True, True, True, True, ["account_id"])

    conn.execute(
        "UPDATE orders SET status = 'paid' WHERE order_id = 3"
    )
    assert rows(conn) == [(10, 3, 2300), (20, 1, 700)]

    conn.execute(
        "REFRESH MATERIALIZED VIEW CONCURRENTLY account_totals"
    )
    assert rows(conn) == [(10, 3, 2300), (20, 2, 5700)]
```

The harness avoids timing-based assertions. It does not sleep and hope a scheduler runs. It controls each state transition explicitly and compares ordered rows. That makes failures useful in CI and review.

We ran this proof end to end on PostgreSQL 15 with Psycopg 3.3.4 on August 10, 2026. Every assertion passed, including the `55000` negative control and concurrent refresh, and PGSandbox deleted the task database afterward. The article pins behavioral references to PostgreSQL 18; run the same proof on every major your application supports.

The definition check deliberately tolerates PostgreSQL adding a table qualifier when it reconstructs the stored query. PostgreSQL documents `pg_get_viewdef()` output as a decompiled reconstruction, so formatting-sensitive equality is not a safe migration assertion.

### Use exact rows for correctness and representative data for performance

Keep the correctness fixture small enough to understand. If the materialized view exists to improve a large aggregation, run performance measurements in a separate lane with representative distributions and stable runner conditions. A fast refresh on five rows says nothing about production cost.

The same separation applies to indexes. The unique index in the harness proves concurrent-refresh eligibility and key uniqueness. It does not prove every read query will use that index. Use the [Postgres EXPLAIN review workflow](/blog/postgres-explain-plan-agent-sql/) for query-plan evidence.

## 3. Test concurrent refresh without false confidence

The strongest concurrent-refresh test has one negative control and one positive path:

1. On a populated materialized view without a qualifying unique index, confirm `REFRESH MATERIALIZED VIEW CONCURRENTLY` is rejected.
2. Apply the migration that creates the all-rows, column-only unique index.
3. Confirm the materialized view is populated.
4. Change source data and prove the stored result remains stale.
5. Run concurrent refresh and assert the exact new result.

The harness asserts SQLSTATE `55000` through Psycopg's `ObjectNotInPrerequisiteState` exception. With autocommit enabled, the failed statement does not strand a surrounding transaction. In a normal transaction, roll it back before continuing; the [PostgreSQL transaction tutorial](https://www.postgresql.org/docs/18/tutorial-transactions.html) explains that a transaction groups statements into an all-or-nothing unit.

Do not call a concurrent refresh lock-free. PostgreSQL's [explicit-locking reference](https://www.postgresql.org/docs/18/explicit-locking.html) shows that concurrent refresh takes an `EXCLUSIVE` lock: plain `SELECT` operations with `ACCESS SHARE` may proceed, while operations that need stronger locks can wait. An application-specific availability test needs at least two coordinated connections: one runs the refresh while another attempts a read under a bounded `statement_timeout`. Use a sufficiently slow, deterministic defining query only in a dedicated integration test. Avoid fragile sleeps in the default suite.

Also avoid an ordering assertion unless the application query has its own `ORDER BY`. PostgreSQL warns that refresh does not guarantee preservation of an `ORDER BY` used in the materialized view's defining query.

### Test the unpopulated state when your migration uses it

Some migrations create materialized views `WITH NO DATA` so deployment can finish before an expensive first population. Test that path explicitly:

```sql
CREATE MATERIALIZED VIEW account_totals AS
SELECT account_id, count(*) AS order_count
FROM orders
GROUP BY account_id
WITH NO DATA;

SELECT ispopulated
FROM pg_matviews
WHERE matviewname = 'account_totals';
```

Expect `ispopulated = false`, and expect scans to fail until a later `REFRESH MATERIALIZED VIEW account_totals`. Concurrent refresh cannot perform that first population. The deployment runbook needs an ordinary refresh before it switches to the concurrent mode.

### Optional: express the content check in pgTAP

Projects that already use [pgTAP](https://pgtap.org/documentation.html) can keep structural and row-set assertions in SQL. `has_materialized_view()` proves existence; it does not prove refresh behavior, so run the same source mutation and refresh sequence around the assertions:

```sql
SELECT plan(2);

SELECT has_materialized_view(
    'public',
    'account_totals',
    'account_totals materialized view exists'
);

SELECT set_eq(
    $$ SELECT account_id, order_count, total_cents
       FROM account_totals $$,
    $$ VALUES (10::bigint, 3::bigint, 2300::bigint),
              (20::bigint, 2::bigint, 5700::bigint) $$,
    'materialized rows match the refreshed source state'
);

SELECT * FROM finish();
```

Use `set_eq()` when order is irrelevant and duplicates are not meaningful, or `bag_eq()` when duplicate multiplicity matters. `results_eq()` is stricter: both queries need compatible types and deterministic ordering when row order is part of the expected result.

## 4. Run the proof in a disposable database

Use the [PGSandbox MCP tool contract](/docs/mcp-tools/) to keep lifecycle authority separate from task SQL:

1. Call `create_database` with a clear owner, label, and TTL.
2. Use `run_repo_command` for the migration and harness so PGSandbox injects `DATABASE_URL` and `PGSANDBOX_DATABASE_URL` without returning credentials in the tool result.
3. Run the repository's real migration as a direct argument array.
4. Execute the Materialized View Refresh Proof through the application test command or a Psycopg harness.
5. Keep output bounded and redact connection strings.
6. Call `delete_database` with the safe database ID and retain only the credential-free result.

The [disposable Postgres integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) shows how to wrap provisioning, command execution, timeout handling, and cleanup around a repository test command. PGSandbox does not install or host PostgreSQL; it creates a database and scoped role on an existing configured server.

The CLI provides the same one-shot lifecycle for a local repository:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint "materialized view proof" \
  --owner "agent-ci" \
  --ttl-minutes 45 \
  --cleanup always \
  -- uv run --with psycopg python tests/test_materialized_views.py
```

Start with the [PGSandbox install guide](/docs/install/), save the harness under the repository's test directory, and run that command against a configured local PostgreSQL 18 profile. The process receives the sandbox URL; the terminal summary can remain credential-free.

If the application refreshes through a job runner, test both layers. The database test proves the SQL contract. A job test should prove that the real job calls the intended refresh mode, reports failure, and does not silently mark stale data as current.

## PR-ready materialized-view proof

A compact review summary should report:

```text
PostgreSQL materialized-view proof
- migration: passed
- relation: public.account_totals
- definition: expected source/filter/grouping confirmed
- initial population: [(10, 2, 2000)]
- stale boundary: source changed; stored result unchanged
- ordinary refresh: [(10, 3, 2300), (20, 1, 700)]
- concurrent index: account_totals_account_id_key
- concurrent refresh: [(10, 3, 2300), (20, 2, 5700)]
- cleanup: sandbox deleted
```

Do not include the database URL or password. The database ID, PostgreSQL major, command result, bounded assertion summary, and cleanup state are enough for review.

## PostgreSQL materialized view testing FAQ

### How do you test a PostgreSQL materialized view?

Inspect the installed definition and indexes, assert its initial stored rows, mutate the source tables, prove the materialized result remains stale, refresh it, and assert the exact new rows. If production uses concurrent refresh, also prove the qualifying unique index, populated state, successful concurrent command, and sandbox cleanup.

### Does PostgreSQL refresh materialized views automatically?

No. PostgreSQL stores the defining query and lets `REFRESH MATERIALIZED VIEW` replace the stored result, but your application or scheduler decides when to run that command. Test the SQL refresh contract separately from the job that schedules or invokes it.

### What index is required for `REFRESH MATERIALIZED VIEW CONCURRENTLY`?

PostgreSQL requires at least one `UNIQUE` index that uses only materialized-view column names and includes every row. A partial index or expression index does not qualify. The materialized view must also already be populated.

### Can you query a materialized view created `WITH NO DATA`?

No. `WITH NO DATA` leaves it unscannable. Run a non-concurrent refresh with data before querying it or using concurrent refresh. The `pg_matviews.ispopulated` column exposes that state for tests and deployment checks.

### Should a materialized-view test compare refresh timing?

Not in the small correctness fixture. Use exact rows and explicit state transitions for correctness. Measure refresh duration separately with representative data, the same PostgreSQL configuration, and a stable runner. A toy fixture cannot support a production performance claim.

### How can you tell when a PostgreSQL materialized view was last refreshed?

Core PostgreSQL does not expose a last-refresh timestamp in `pg_matviews`. If the time matters operationally, record each refresh attempt and completion in an application-owned table, job log, or metrics system. Test that tracking beside the job that invokes refresh; do not infer freshness from the materialized view's modification time.

## Sources

- [PostgreSQL 18: Materialized Views](https://www.postgresql.org/docs/18/rules-materializedviews.html)
- [PostgreSQL 18: CREATE MATERIALIZED VIEW](https://www.postgresql.org/docs/18/sql-creatematerializedview.html)
- [PostgreSQL 18: REFRESH MATERIALIZED VIEW](https://www.postgresql.org/docs/18/sql-refreshmaterializedview.html)
- [PostgreSQL 18: `pg_matviews`](https://www.postgresql.org/docs/18/view-pg-matviews.html)
- [PostgreSQL 18: `pg_index`](https://www.postgresql.org/docs/18/catalog-pg-index.html)
- [PostgreSQL 18: Explicit Locking](https://www.postgresql.org/docs/18/explicit-locking.html)

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "FAQPage",
  "mainEntity": [
    {"@type": "Question", "name": "How do you test a PostgreSQL materialized view?", "acceptedAnswer": {"@type": "Answer", "text": "Inspect the installed definition and indexes, assert initial stored rows, mutate source tables, prove the result remains stale, refresh it, and assert exact new rows. Also test concurrent-refresh prerequisites when production uses that mode."}},
    {"@type": "Question", "name": "Does PostgreSQL refresh materialized views automatically?", "acceptedAnswer": {"@type": "Answer", "text": "No. An application or scheduler must invoke REFRESH MATERIALIZED VIEW."}},
    {"@type": "Question", "name": "What index is required for REFRESH MATERIALIZED VIEW CONCURRENTLY?", "acceptedAnswer": {"@type": "Answer", "text": "At least one UNIQUE index using only column names and including every row. Partial and expression indexes do not qualify, and the view must already be populated."}},
    {"@type": "Question", "name": "Can you query a materialized view created WITH NO DATA?", "acceptedAnswer": {"@type": "Answer", "text": "No. It remains unscannable until a non-concurrent refresh populates it."}},
    {"@type": "Question", "name": "Should a materialized-view test compare refresh timing?", "acceptedAnswer": {"@type": "Answer", "text": "Keep timing out of the small correctness fixture. Measure performance separately with representative data and stable runner conditions."}},
    {"@type": "Question", "name": "How can you tell when a PostgreSQL materialized view was last refreshed?", "acceptedAnswer": {"@type": "Answer", "text": "Core PostgreSQL does not expose a last-refresh timestamp in pg_matviews. Record refresh attempts and completions in an application-owned table, job log, or metrics system."}}
  ]
}
</script>
