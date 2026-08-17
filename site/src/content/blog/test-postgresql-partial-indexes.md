---
title: "How to Test PostgreSQL Partial Indexes"
excerpt: "Prove the installed predicate, indexed row set, positive and negative plan paths, uniqueness behavior, data transitions, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-06"
updatedAt: "2026-08-06T06:00:00Z"
tags: ["Postgres", "partial indexes", "integration testing", "query plans", "coding agents"]
category: "Engineering"
metaTitle: "How to Test PostgreSQL Partial Indexes"
metaDescription: "Test PostgreSQL partial indexes with catalog checks, positive and negative plans, uniqueness transitions, exact final state, and cleanup."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/test-postgresql-partial-indexes/"
heroImageUrl: ""
featured: false
sortOrder: 157
---
Test PostgreSQL partial indexes by proving five boundaries: the exact predicate installed by the migration, which rows qualify for the index, whether an eligible query can use it, whether an ineligible query cannot use it, and how writes crossing the predicate affect uniqueness. Run those checks against representative data in a disposable database, then verify the final state and cleanup.

A single `EXPLAIN` screenshot is not enough. It can pass because the fixture is unusually small, the planner statistics are stale, or the query accidentally repeats the predicate while the application query does not. A unique partial index can also enforce the correct read path but fail on a status transition your test never exercised.

This guide packages the checks into a five-part **Partial Index Proof**: declaration, membership, eligible plan, negative control, and mutation boundary. PGSandbox MCP supplies a task-scoped database and restricted role on your configured PostgreSQL server. PostgreSQL catalogs, JSON query plans, SQLSTATE assertions, and exact final rows supply the evidence.

*Published and last updated August 6, 2026.*

The complete workflow is:

1. Apply the real migration and inspect the index definition, predicate, uniqueness, and validity.
2. Seed representative rows on both sides of the predicate, then run `ANALYZE`.
3. Prove an eligible query can use the named partial index.
4. Prove a query outside the predicate does not use that index.
5. Exercise inserts and updates that cross the predicate, assert SQLSTATE `23505`, verify exact final rows, and delete the sandbox.

## In this guide

- [Understand partial indexes](#what-a-postgresql-partial-index-proves)
- [Use the Partial Index Proof](#the-partial-index-proof)
- [Inspect the installed predicate](#1-inspect-the-installed-partial-index)
- [Run a deterministic harness](#2-run-a-deterministic-partial-index-test)
- [Keep plan assertions honest](#3-avoid-brittle-or-misleading-plan-tests)
- [Run with PGSandbox](#4-run-the-proof-in-a-disposable-database)
- [Record review evidence](#pr-ready-partial-index-proof)
- [Answer common questions](#postgresql-partial-index-testing-faq)

## What a PostgreSQL partial index proves

A partial index stores entries only for rows that satisfy its `WHERE` predicate. PostgreSQL's current [partial-index documentation](https://www.postgresql.org/docs/current/indexes-partial.html) describes two separate jobs such an index can perform: reduce the indexed row set for selected queries, and enforce uniqueness over only the qualifying rows.

Those jobs create different test obligations. A read-oriented partial index needs evidence that the target application query implies the index predicate and that the planner can choose the index under representative conditions. A unique partial index also needs behavior tests for rows entering, leaving, and remaining outside the indexed set.

PostgreSQL decides whether a query may use a partial index at planning time. The query condition must mathematically imply the index predicate. PostgreSQL recognizes straightforward matches and some simple inequalities, but it is not a general theorem prover. The documentation calls out a practical consequence: a parameterized clause cannot imply a fixed partial-index predicate for every possible parameter value.

For example, this index covers active jobs:

```sql
CREATE UNIQUE INDEX jobs_active_external_id_unique
ON jobs (tenant_id, external_id)
WHERE status IN ('queued', 'running');
```

This query states the qualifying condition directly and is eligible to use the index:

```sql
SELECT id
FROM jobs
WHERE tenant_id = 7
  AND external_id = 'active-7'
  AND status IN ('queued', 'running');
```

A query for `status = 'completed'` is outside the indexed set. A query that omits `status` is not allowed to assume the requested row is active. Both are valuable negative controls.

### Index existence, eligibility, and selection are different claims

Keep these three claims separate in a review:

| Claim | What proves it |
| --- | --- |
| The migration created the intended index | `pg_index`, `pg_get_indexdef`, and `pg_get_expr` |
| A query is eligible to use the index | Its predicates imply the installed partial predicate |
| The planner selected the index for this fixture | A machine-readable `EXPLAIN` plan names the index |

An eligible query does not have to use an index for every data set. PostgreSQL may correctly choose a sequential scan for a small table or a query that returns much of the relation. The [PostgreSQL EXPLAIN guide](https://www.postgresql.org/docs/current/using-explain.html) warns that plans from toy-sized tables should not be extrapolated to materially different data volumes.

## The Partial Index Proof

A reviewable partial-index test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Declaration | What did the migration install? | Exact definition, predicate, access method, uniqueness, readiness, and validity |
| Membership | Which fixture rows satisfy the predicate? | Counts and exact keys on both sides of the predicate |
| Eligible plan | Can the real query shape use the index? | JSON plan containing the expected index name after representative seeding and `ANALYZE` |
| Negative control | Does an ineligible query avoid that partial index? | JSON plan without the partial index name |
| Mutation boundary | What happens when rows enter or leave the indexed set? | Successful controls, SQLSTATE `23505`, and exact committed final rows |

This separation is the information gain of the guide. It prevents a green plan assertion from hiding the wrong predicate, a nonrepresentative fixture, or broken write semantics. It also gives a reviewer a compact answer to the important question: did the migration install the intended contract, or did one lucky query happen to run quickly?

## 1. Inspect the installed partial index

Apply the repository's real migration first. Recreating similar DDL in the test proves the example, not the migration under review. The [database migration testing workflow](/blog/database-migration-testing-agent-pr/) covers applying the real upgrade path before collecting schema and behavior evidence.

Query the catalogs by the schema-qualified index name:

```sql
SELECT
    idx_ns.nspname AS index_schema,
    idx.relname AS index_name,
    am.amname AS access_method,
    i.indisunique,
    i.indisready,
    i.indisvalid,
    pg_get_indexdef(i.indexrelid) AS index_definition,
    pg_get_expr(i.indpred, i.indrelid) AS index_predicate
FROM pg_catalog.pg_index AS i
JOIN pg_catalog.pg_class AS idx
  ON idx.oid = i.indexrelid
JOIN pg_catalog.pg_namespace AS idx_ns
  ON idx_ns.oid = idx.relnamespace
JOIN pg_catalog.pg_am AS am
  ON am.oid = idx.relam
WHERE i.indexrelid =
      'public.jobs_active_external_id_unique'::regclass;
```

The current [`pg_index` catalog reference](https://www.postgresql.org/docs/current/catalog-pg-index.html) defines `indpred` as the expression tree for a partial-index predicate. It also distinguishes `indisready`, which controls whether writes maintain the index, from `indisvalid`, which controls whether queries may safely use it. A test that checks only `pg_indexes.indexdef` can miss a concurrently built index left invalid by an interrupted migration.

Assert the normalized predicate and key order, not only the index name. For the example above, the contract includes all of these details:

- B-tree access method.
- Unique enforcement.
- Key order `(tenant_id, external_id)`.
- Predicate `status IN ('queued', 'running')`.
- Ready and valid state.

The predicate should match the application's real active-state definition. If the application later adds `retrying` but the migration does not, plan and uniqueness behavior will diverge even though the index remains valid.

## 2. Run a deterministic partial-index test

The following Psycopg harness creates a reproducible fixture. In an application repository, replace `install_fixture()` with the real migration command and keep the remaining catalog, plan, error, and final-state assertions against the migrated table.

```python
import os

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]
INDEX_NAME = "jobs_active_external_id_unique"


def connect():
    return psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    )


def install_fixture(conn):
    conn.execute("DROP TABLE IF EXISTS jobs")
    conn.execute(
        """
        CREATE TABLE jobs (
            id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            tenant_id integer NOT NULL,
            external_id text NOT NULL,
            status text NOT NULL CHECK (
                status IN ('queued', 'running', 'completed')
            )
        )
        """
    )
    conn.execute(
        """
        CREATE UNIQUE INDEX jobs_active_external_id_unique
        ON jobs (tenant_id, external_id)
        WHERE status IN ('queued', 'running')
        """
    )


def seed_representative_rows(conn):
    conn.execute(
        """
        INSERT INTO jobs (tenant_id, external_id, status)
        SELECT
            (n % 50) + 1,
            'completed-' || n,
            'completed'
        FROM generate_series(1, 20000) AS n
        """
    )
    conn.execute(
        """
        INSERT INTO jobs (tenant_id, external_id, status)
        VALUES
            (7, 'active-7', 'queued'),
            (8, 'active-8', 'running')
        """
    )
    conn.execute("ANALYZE jobs")


def declaration(conn):
    return conn.execute(
        """
        SELECT
            am.amname,
            i.indisunique,
            i.indisready,
            i.indisvalid,
            pg_get_indexdef(i.indexrelid),
            pg_get_expr(i.indpred, i.indrelid)
        FROM pg_catalog.pg_index AS i
        JOIN pg_catalog.pg_class AS idx
          ON idx.oid = i.indexrelid
        JOIN pg_catalog.pg_am AS am
          ON am.oid = idx.relam
        WHERE i.indexrelid =
              'public.jobs_active_external_id_unique'::regclass
        """
    ).fetchone()


def explain(conn, sql, params):
    row = conn.execute(
        "EXPLAIN (FORMAT JSON, COSTS FALSE) " + sql,
        params,
    ).fetchone()
    return row[0][0]["Plan"]


def walk_plan(node):
    yield node
    for child in node.get("Plans", []):
        yield from walk_plan(child)


def index_names(plan):
    return {
        node["Index Name"]
        for node in walk_plan(plan)
        if "Index Name" in node
    }


def expect_unique_violation(conn, sql, params):
    try:
        conn.execute(sql, params)
    except errors.UniqueViolation as exc:
        assert exc.sqlstate == "23505"
        assert exc.diag.constraint_name == INDEX_NAME
        return
    raise AssertionError("expected partial unique-index violation")


with connect() as conn:
    install_fixture(conn)
    seed_representative_rows(conn)

    method, unique, ready, valid, definition, predicate = declaration(conn)
    assert method == "btree"
    assert unique and ready and valid
    assert "(tenant_id, external_id)" in definition
    assert "status" in predicate
    assert "queued" in predicate and "running" in predicate

    qualifying = conn.execute(
        """
        SELECT count(*)
        FROM jobs
        WHERE status IN ('queued', 'running')
        """
    ).fetchone()[0]
    assert qualifying == 2

    eligible = explain(
        conn,
        """
        SELECT id FROM jobs
        WHERE tenant_id = %s
          AND external_id = %s
          AND status IN ('queued', 'running')
        """,
        (7, "active-7"),
    )
    assert INDEX_NAME in index_names(eligible)

    ineligible = explain(
        conn,
        """
        SELECT id FROM jobs
        WHERE tenant_id = %s
          AND external_id = %s
          AND status = 'completed'
        """,
        (7, "completed-6"),
    )
    assert INDEX_NAME not in index_names(ineligible)

    expect_unique_violation(
        conn,
        """
        INSERT INTO jobs (tenant_id, external_id, status)
        VALUES (%s, %s, 'running')
        """,
        (7, "active-7"),
    )

    conn.execute(
        """
        INSERT INTO jobs (tenant_id, external_id, status)
        VALUES (7, 'active-7', 'completed')
        """
    )
    expect_unique_violation(
        conn,
        """
        UPDATE jobs
        SET status = 'queued'
        WHERE tenant_id = 7
          AND external_id = 'active-7'
          AND status = 'completed'
        """,
        (),
    )

    final_rows = conn.execute(
        """
        SELECT tenant_id, external_id, status
        FROM jobs
        WHERE external_id = 'active-7'
        ORDER BY status
        """
    ).fetchall()
    assert [tuple(row) for row in final_rows] == [
        (7, "active-7", "completed"),
        (7, "active-7", "queued"),
    ]

print("partial-index proof passed")
```

PostgreSQL recommends running `ANALYZE` after substantial data changes when fresh planner statistics are needed. The current [`ANALYZE` reference](https://www.postgresql.org/docs/current/sql-analyze.html) also notes that its samples are approximate, so cost estimates can move slightly between runs. This harness asserts the selected index name, not exact cost or timing numbers.

The duplicate checks use SQLSTATE `23505`. PostgreSQL's [error-code appendix](https://www.postgresql.org/docs/current/errcodes-appendix.html) recommends branching on stable codes rather than localized error text and maps `23505` to `unique_violation`. Psycopg exposes the associated index or constraint name separately through diagnostic fields.

### Test every predicate transition your application can make

For a unique partial index, cover at least these transitions:

| Transition | Expected result |
| --- | --- |
| Outside to outside | No partial uniqueness check |
| Outside to inside without conflict | Row enters the indexed set |
| Outside to inside with conflict | `23505`; original row remains outside |
| Inside to outside | Key becomes available to another qualifying row |
| Inside to inside with same key | Existing index entry remains protected |
| Qualifying insert with duplicate key | `23505` naming the partial unique index |

These cases are more valuable than checking index size. They prove the business rule stays correct as rows move through the application's state machine.

## 3. Avoid brittle or misleading plan tests

Do not make a CI test fail because an exact `EXPLAIN` text block changed. Plans are trees, and PostgreSQL may change node details across versions, data distributions, or statistics samples. Use `FORMAT JSON`, walk the tree, and assert only the contract you need: the expected index appears for the eligible query and does not appear for the negative control.

Do not set `enable_seqscan = off` in the main proof. That setting can demonstrate that an index path exists, but it does not prove the planner would choose it under normal configuration. If you use it during diagnosis, label the result as forced and keep it out of the passing production assertion.

Do not seed ten rows and treat an index scan as mandatory. A sequential scan may be the correct plan when the whole table fits on one page. Use a fixture large and skewed enough to resemble the decision the migration was designed to improve. If production behavior depends on tenant skew, status distribution, or a range cutoff, model that distribution explicitly.

Do not use `EXPLAIN ANALYZE` on a modifying statement unless the test wraps it in a transaction and rolls it back. PostgreSQL executes the statement when `ANALYZE` is requested. For simple eligibility checks, plain `EXPLAIN (FORMAT JSON)` avoids side effects and makes the proof cheaper.

The existing [Postgres EXPLAIN plan guide](/blog/postgres-explain-plan-agent-sql/) covers bounded plan evidence for agent SQL review. The partial-index proof adds the schema predicate, negative control, and write-transition boundaries that a general query-plan check does not cover.

## 4. Run the proof in a disposable database

Save the harness as `tests/postgres_partial_index_proof.py`, replace the fixture installer with the repository's real migration path, and run it through a one-shot PGSandbox session:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint "partial index proof" \
  --ttl-minutes 30 \
  --owner "agent-pr-483" \
  --label "suite=partial-indexes" \
  --cleanup always \
  --result-format json \
  --env-var PGSANDBOX_DATABASE_URL \
  -- uv run --with 'psycopg[binary]' \
       python tests/postgres_partial_index_proof.py
```

`with-database` creates a tracked database and scoped login role, injects the credential into the child process, captures bounded credential-redacted output, and applies the selected cleanup policy. The [one-shot integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) explains the full session result contract, and the [PGSandbox MCP tool reference](/docs/mcp-tools/) covers the underlying lifecycle operations.

Do not print `PGSANDBOX_DATABASE_URL` in CI logs or PR comments. Record the PostgreSQL major or profile, migration identifier, predicate summary, qualifying-row count, positive and negative plan results, mutation assertions, child exit status, and cleanup result.

### Interpret each failed boundary

| Failed assertion | Likely problem |
| --- | --- |
| Definition or predicate differs | Migration drift, wrong schema, wrong key order, or stale active-state definition |
| `indisready` or `indisvalid` is false | Interrupted or incomplete concurrent-index migration |
| Qualifying count differs | Fixture does not model the predicate or setup leaked state |
| Eligible query misses the index | Query does not imply the predicate, statistics are stale, or the fixture is not representative |
| Negative control uses the index | Test inspected the wrong index or query condition differs from the intended control |
| Duplicate qualifying row succeeds | Index is not unique, predicate excludes the row, or migration did not run |
| Outside-to-inside update succeeds despite a conflict | Predicate or application state transition differs from the expected contract |
| Final rows differ | A failed statement leaked state or a control case changed the wrong row |

Keep declaration failure separate from plan failure. There is little value in interpreting a query plan when the installed index is already not the object the migration intended to create.

## PR-ready partial-index proof

Keep the review note compact and bounded:

```text
Partial-index proof
- target: PostgreSQL 18, PGSandbox managed-local profile
- migration: applied the repository's real upgrade path
- declaration: unique btree (tenant_id, external_id), expected active predicate
- state: ready=true, valid=true
- membership: 2 qualifying rows, 20,000 negative-control rows
- eligible plan: jobs_active_external_id_unique selected
- negative plan: partial index absent
- mutation: duplicate insert and outside-to-inside conflict returned 23505
- final state: exact active/completed rows matched
- child exit: 0
- cleanup: sandbox deleted
```

Link the migration and test file. Do not paste full database URLs, an entire JSON plan, or thousands of seeded rows. The [bounded SQL evidence guide](/blog/postgres-run-sql-bounded-results/) shows how to keep agent-generated proof small enough for a reviewer to verify.

## PostgreSQL partial index testing FAQ

### How do I check whether a PostgreSQL index is partial?

Query `pg_index.indpred`. A non-null predicate expression means the index is partial. Use `pg_get_expr(indpred, indrelid)` to render the stored expression and `pg_get_indexdef(indexrelid)` to inspect the complete index definition.

### Why is PostgreSQL not using my partial index?

The query may not imply the index predicate, its parameters may prevent planning-time implication, the table may be too small, or planner statistics may be stale. Check the installed predicate, repeat the predicate in the real query shape where appropriate, seed representative data, run `ANALYZE`, and inspect a JSON plan without disabling sequential scans.

### Should a test assert exact EXPLAIN costs?

No. Costs and row estimates can shift with PostgreSQL versions, configuration, and sampled statistics. Assert the small structural contract you need, such as whether a named index appears in the plan tree, and verify result correctness separately.

### How do I test a unique partial index?

Test duplicate rows both inside and outside the predicate, then test updates that move rows into and out of the indexed set. Qualifying conflicts should return SQLSTATE `23505`; nonqualifying duplicates should remain allowed if no other constraint forbids them.

### Can a prepared query use a partial index?

It depends on whether PostgreSQL can prove at planning time that the query condition implies the predicate. A generic parameter such as `x < $1` cannot imply a fixed predicate such as `x < 2` for every parameter value. Test the same prepared-query shape and plan-cache behavior your application uses.

### Why use a disposable database for partial-index tests?

The proof applies real DDL, seeds a planner-relevant distribution, deliberately triggers uniqueness errors, and may leave an invalid object when a migration fails. A disposable [Postgres database sandbox](/blog/what-is-database-sandbox/) keeps that work task-scoped and returns an explicit cleanup result.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Article",
      "headline": "How to Test PostgreSQL Partial Indexes",
      "description": "Test PostgreSQL partial indexes with catalog checks, positive and negative plans, uniqueness transitions, exact final state, and cleanup.",
      "datePublished": "2026-08-06",
      "dateModified": "2026-08-06",
      "author": {"@type": "Organization", "name": "PGSandbox Team"},
      "publisher": {"@type": "Organization", "name": "PGSandbox"},
      "mainEntityOfPage": "https://pgsandbox.lvtd.dev/blog/test-postgresql-partial-indexes/"
    },
    {
      "@type": "HowTo",
      "name": "How to test PostgreSQL partial indexes",
      "description": "Inspect the installed predicate, seed representative rows, verify eligible and ineligible plans, test uniqueness transitions, and clean up the disposable database.",
      "step": [
        {"@type": "HowToStep", "name": "Inspect the index", "text": "Apply the real migration and inspect the definition, predicate, uniqueness, readiness, and validity."},
        {"@type": "HowToStep", "name": "Seed representative rows", "text": "Create rows on both sides of the predicate and run ANALYZE."},
        {"@type": "HowToStep", "name": "Verify the eligible plan", "text": "Run JSON EXPLAIN for the real qualifying query and confirm the expected index appears."},
        {"@type": "HowToStep", "name": "Run a negative control", "text": "Explain a query outside the predicate and confirm the partial index is absent."},
        {"@type": "HowToStep", "name": "Test mutations and cleanup", "text": "Exercise rows entering and leaving the predicate, assert SQLSTATE 23505 where required, verify final rows, and delete the sandbox."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do I check whether a PostgreSQL index is partial?", "acceptedAnswer": {"@type": "Answer", "text": "Query pg_index.indpred. A non-null predicate means the index is partial. Render it with pg_get_expr and inspect the complete definition with pg_get_indexdef."}},
        {"@type": "Question", "name": "Why is PostgreSQL not using my partial index?", "acceptedAnswer": {"@type": "Answer", "text": "The query may not imply the predicate, parameters may prevent planning-time implication, the table may be too small, or statistics may be stale. Verify the installed predicate and inspect a representative JSON plan after ANALYZE."}},
        {"@type": "Question", "name": "Should a test assert exact EXPLAIN costs?", "acceptedAnswer": {"@type": "Answer", "text": "No. Costs and estimates vary with versions, configuration, and sampled statistics. Assert only the required plan structure and verify result correctness separately."}},
        {"@type": "Question", "name": "How do I test a unique partial index?", "acceptedAnswer": {"@type": "Answer", "text": "Test duplicates inside and outside the predicate plus updates that cross the predicate. Qualifying conflicts should return SQLSTATE 23505; nonqualifying duplicates remain allowed unless another constraint rejects them."}},
        {"@type": "Question", "name": "Why use a disposable database for partial-index tests?", "acceptedAnswer": {"@type": "Answer", "text": "The proof applies real DDL, seeds representative data, and deliberately triggers failures. A disposable database keeps the work task-scoped and provides an explicit cleanup result."}}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "Home", "item": "https://pgsandbox.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test PostgreSQL Partial Indexes", "item": "https://pgsandbox.lvtd.dev/blog/test-postgresql-partial-indexes/"}
      ]
    }
  ]
}
</script>
