---
title: "How to Test PostgreSQL Generated Columns"
excerpt: "Prove the installed expression and storage kind, rejected direct writes, recomputation, trigger timing, version behavior, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-05"
updatedAt: "2026-08-05T06:00:00Z"
tags: ["Postgres", "generated columns", "integration testing", "triggers", "coding agents"]
category: "Engineering"
metaTitle: "How to Test PostgreSQL Generated Columns"
metaDescription: "Test PostgreSQL generated columns with catalog checks, direct-write rejection, recomputation, trigger timing, version checks, and cleanup."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/test-postgresql-generated-columns/"
heroImageUrl: ""
featured: false
sortOrder: 156
---
Test PostgreSQL generated columns by proving five boundaries: the exact expression and storage kind installed by the migration, rejection of direct writes, recomputation after base-column changes, interaction with `BEFORE` and `AFTER` triggers, and behavior on every supported PostgreSQL major. Run those checks against the real migration in a disposable database, then verify cleanup.

A test that selects one expected value proves only one example. It can still pass when the migration installed the wrong expression, changed a stored column to virtual on PostgreSQL 18, accepts an unsupported server version, or interacts incorrectly with a trigger that rewrites a base column.

This guide packages the checks into a five-part **Generation Boundary Proof**: declaration, write protection, derivation, execution boundary, and compatibility. PGSandbox MCP supplies a task-scoped database and restricted role on your configured PostgreSQL server. PostgreSQL catalogs and application assertions supply the proof.

*Published and last updated August 5, 2026.*

The complete workflow is:

1. Apply the repository's real migration and inspect the generation expression plus stored/virtual kind.
2. Prove an explicit value is rejected with SQLSTATE `428C9`, while `DEFAULT` is accepted.
3. Insert and update fixed base values, then assert exact generated values and final rows.
4. If triggers touch the table, prove which values exist before and after generation.
5. Run the proof on each supported PostgreSQL major and delete the disposable database.

## In this guide

- [Understand generated columns](#what-postgresql-generated-columns-do)
- [Use the Generation Boundary Proof](#the-generation-boundary-proof)
- [Inspect the installed declaration](#1-inspect-the-installed-generated-column)
- [Run a deterministic harness](#2-run-a-deterministic-generated-column-test)
- [Test trigger timing](#3-test-generated-columns-with-before-and-after-triggers)
- [Test the version boundary](#4-test-stored-and-virtual-columns-on-supported-postgresql-versions)
- [Run with PGSandbox](#5-run-the-proof-in-a-disposable-database)
- [Record review evidence](#pr-ready-generated-column-proof)
- [Answer common questions](#postgresql-generated-column-testing-faq)

## What PostgreSQL generated columns do

A generated column derives its value from other columns in the same row. PostgreSQL owns that value: an application changes the base columns, and PostgreSQL computes the generated result from the declared expression.

PostgreSQL 18 supports two kinds. A **stored** generated column is computed during `INSERT` or `UPDATE` and occupies table storage. A **virtual** generated column is computed when read and occupies no storage. The current [generated-column documentation](https://www.postgresql.org/docs/18/ddl-generated-columns.html) also records a version-sensitive change: PostgreSQL 18 makes virtual generation the default when `VIRTUAL` or `STORED` is omitted. PostgreSQL 12 through 17 support stored generated columns, so portable migrations should state `STORED` explicitly when that is the intended contract.

Generated columns differ from defaults in ways a test should expose:

| Property | Column default | Generated column |
| --- | --- | --- |
| Evaluation | Once when an omitted value is inserted | On every relevant write for stored columns; on read for virtual columns |
| May reference sibling columns | No | Yes, within the same row |
| Caller may override the value | Yes | No; only `DEFAULT` is accepted as an explicit placeholder |
| Volatile expressions | Allowed | Not allowed; generation expressions use immutable functions |
| May reference another generated column | Not applicable | No |

The restriction matters during review. A migration may look correct while using a function PostgreSQL considers volatile, referencing another generated column, or relying on a user-defined function that a PostgreSQL 18 virtual column cannot use. A migration test should fail at DDL application before the behavior cases run.

### Generated values have an execution boundary

Generated values are not available at every point in a write. PostgreSQL documents that base-column changes made by a row-level `BEFORE` trigger affect the generated result. Stored generated columns are then computed before row-level `AFTER` triggers, so an `AFTER` trigger can inspect the final stored value.

Virtual columns are different: PostgreSQL does not compute them while triggers fire. Trigger code should not read a virtual generated value from `NEW` or `OLD`. This makes a migration from stored to virtual more than a storage choice when trigger functions depend on the generated field.

The companion [PostgreSQL trigger testing guide](/blog/test-postgresql-triggers/) covers the broader declaration, firing-matrix, side-effect, and rollback checks. Add the generation boundary here when the trigger reads or changes a row involved in a generation expression.

## The Generation Boundary Proof

A reviewable generated-column test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Declaration | What did the migration install? | Exact column identity, expression, and `stored` or `virtual` kind |
| Write protection | Does PostgreSQL remain the only writer? | SQLSTATE `428C9` for an explicit value; successful `DEFAULT` control |
| Derivation | Does every relevant base-column change produce the exact result? | Fixed insert/update fixtures and ordered final rows |
| Execution boundary | Do trigger changes feed generation at the documented time? | `BEFORE` base rewrite plus stored-value observation in `AFTER` |
| Compatibility | Does the DDL mean the same thing on every supported major? | Per-major migration and behavior result, with explicit handling of PostgreSQL 18 virtual columns |

This framework is the information gain of the guide. Most examples stop after `SELECT` returns a calculated value. The proof separates schema identity, write authority, value derivation, trigger timing, and server-version behavior so one passing assertion cannot hide a different broken boundary.

## 1. Inspect the installed generated column

Run the repository's real migration first. Recreating a similar table inside the test proves the example DDL, not the migration under review. The [database migration testing workflow](/blog/database-migration-testing-agent-pr/) shows how to apply the real upgrade path and retain compact evidence before an agent opens a PR.

Start with the portable information-schema view:

```sql
SELECT
    table_schema,
    table_name,
    column_name,
    data_type,
    is_generated,
    generation_expression
FROM information_schema.columns
WHERE table_schema = 'public'
  AND table_name = 'generated_orders'
  AND column_name = 'net_amount';
```

The expected `is_generated` value is `ALWAYS`. Assert the normalized expression rather than checking only that it is non-null. A missing tax term, an unintended cast, or changed rounding rule is a schema change even if the column is still generated.

`information_schema.columns` does not distinguish stored from virtual. For that PostgreSQL-specific boundary, inspect `pg_attribute.attgenerated`. The PostgreSQL 18 [`pg_attribute` catalog](https://www.postgresql.org/docs/18/catalog-pg-attribute.html) defines `s` as stored and `v` as virtual:

```sql
SELECT
    n.nspname AS schema_name,
    c.relname AS table_name,
    a.attname AS column_name,
    a.attgenerated AS generation_kind,
    pg_get_expr(d.adbin, d.adrelid) AS generation_expression
FROM pg_catalog.pg_attribute AS a
JOIN pg_catalog.pg_class AS c
  ON c.oid = a.attrelid
JOIN pg_catalog.pg_namespace AS n
  ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_attrdef AS d
  ON d.adrelid = a.attrelid
 AND d.adnum = a.attnum
WHERE n.nspname = 'public'
  AND c.relname = 'generated_orders'
  AND a.attname = 'net_amount'
  AND a.attnum > 0
  AND NOT a.attisdropped;
```

Match schema, table, and column explicitly. A loose query can pass against an old test table or another schema on the search path.

## 2. Run a deterministic generated-column test

The following Psycopg harness creates a small stored-column fixture. In a real repository, replace `install_fixture()` with the actual migration command and keep the remaining assertions against the migrated table.

```python
import os
from decimal import Decimal

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]


def connect():
    return psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    )


def install_fixture(conn):
    conn.execute("DROP TABLE IF EXISTS generated_orders")
    conn.execute(
        """
        CREATE TABLE generated_orders (
            id integer PRIMARY KEY,
            unit_price numeric(12, 2) NOT NULL,
            quantity integer NOT NULL,
            discount numeric(12, 2) NOT NULL DEFAULT 0,
            net_amount numeric(12, 2)
                GENERATED ALWAYS AS (
                    (unit_price * quantity) - discount
                ) STORED
        )
        """
    )


def declaration(conn):
    return conn.execute(
        """
        SELECT
            i.is_generated,
            i.generation_expression,
            a.attgenerated
        FROM information_schema.columns AS i
        JOIN pg_catalog.pg_namespace AS n
          ON n.nspname = i.table_schema
        JOIN pg_catalog.pg_class AS c
          ON c.relnamespace = n.oid
         AND c.relname = i.table_name
        JOIN pg_catalog.pg_attribute AS a
          ON a.attrelid = c.oid
         AND a.attname = i.column_name
        WHERE i.table_schema = 'public'
          AND i.table_name = 'generated_orders'
          AND i.column_name = 'net_amount'
        """
    ).fetchone()


def expect_direct_write_rejection(conn):
    try:
        conn.execute(
            """
            INSERT INTO generated_orders
                (id, unit_price, quantity, discount, net_amount)
            VALUES (1, 10.00, 3, 2.00, 999.00)
            """
        )
    except errors.GeneratedAlways as exc:
        assert exc.sqlstate == "428C9"
        return exc.sqlstate
    raise AssertionError("expected SQLSTATE 428C9")


def prove_derivation(conn):
    inserted = conn.execute(
        """
        INSERT INTO generated_orders
            (id, unit_price, quantity, discount, net_amount)
        VALUES (2, 10.00, 3, 2.00, DEFAULT)
        RETURNING id, unit_price, quantity, discount, net_amount
        """
    ).fetchone()
    assert tuple(inserted) == (
        2,
        Decimal("10.00"),
        3,
        Decimal("2.00"),
        Decimal("28.00"),
    )

    updated = conn.execute(
        """
        UPDATE generated_orders
        SET quantity = 5, discount = 7.50
        WHERE id = 2
        RETURNING id, unit_price, quantity, discount, net_amount
        """
    ).fetchone()
    assert tuple(updated) == (
        2,
        Decimal("10.00"),
        5,
        Decimal("7.50"),
        Decimal("42.50"),
    )


def final_state(conn):
    rows = conn.execute(
        """
        SELECT id, unit_price, quantity, discount, net_amount
        FROM generated_orders
        ORDER BY id
        """
    ).fetchall()
    return [tuple(row) for row in rows]


with connect() as conn:
    install_fixture(conn)
    generated, expression, kind = declaration(conn)
    assert generated == "ALWAYS"
    assert kind == "s"
    assert "unit_price" in expression
    assert "quantity" in expression
    assert "discount" in expression

    assert expect_direct_write_rejection(conn) == "428C9"
    prove_derivation(conn)
    assert final_state(conn) == [(
        2,
        Decimal("10.00"),
        5,
        Decimal("7.50"),
        Decimal("42.50"),
    )]

print("generated-column-proof: ok")
```

The failed direct insert uses a different fixed id from the valid row. The final-state assertion therefore proves both that PostgreSQL rejected caller authority over the generated value and that no partial row leaked from the failing statement.

Psycopg maps SQLSTATE `428C9` to `GeneratedAlways`. Checking the SQLSTATE as well as the exception class keeps the assertion stable across message wording and localization. PostgreSQL's [`INSERT` reference](https://www.postgresql.org/docs/18/sql-insert.html) permits `DEFAULT` for a generated column, but that keyword only requests normal computation; it does not override the expression.

### Cover expression edge cases, not random values

Choose fixtures at semantic boundaries in the real expression:

- nullability transitions if any base input can be null;
- zero, negative, and maximum accepted values for numeric expressions;
- rounding boundaries for `numeric` calculations;
- time-zone or collation cases when immutable built-in functions depend on those types;
- every branch of a `CASE` expression;
- updates that change one dependency at a time, plus an unrelated-column update.

Use fixed expected results. Randomized property tests can add coverage, but they should not replace a small table of reviewable examples tied to the business rule.

## 3. Test generated columns with BEFORE and AFTER triggers

Add a trigger case only when the migrated table has triggers or the change introduces one. The goal is to prove ordering, not merely that both features exist.

For a stored generated column, use this sequence:

1. A row-level `BEFORE` trigger changes a base input such as `discount`.
2. The generated expression computes from the trigger-adjusted base row.
3. A row-level `AFTER` trigger records the final generated value in a test audit table.
4. The test asserts the returned row and the audit row contain the same exact result.

PostgreSQL's [trigger behavior documentation](https://www.postgresql.org/docs/18/trigger-definition.html) states that stored generated columns are computed after `BEFORE` triggers and before `AFTER` triggers. Do not read the new generated value in a `BEFORE` trigger. It has not been computed yet, and changes assigned directly to it will be overwritten.

For a virtual generated column, do not design trigger logic that reads the virtual field from `NEW` or `OLD`. PostgreSQL never computes virtual generated values while triggers fire. Recompute from the base columns inside the trigger only when duplicating that rule is an intentional, reviewed tradeoff; otherwise move the dependent behavior outside the trigger.

This trigger test catches a subtle regression that a normal select cannot. A stored-to-virtual migration may keep query results correct while invalidating an `AFTER` trigger that previously consumed the stored generated value.

## 4. Test stored and virtual columns on supported PostgreSQL versions

PostgreSQL 18 introduced virtual generated columns and made virtual the default. Treat that as a compatibility boundary in any project supporting more than one major.

Use an explicit matrix:

| Migration intent | PostgreSQL 12-17 | PostgreSQL 18 |
| --- | --- | --- |
| Stored value | Declare `STORED`; expect catalog kind `s` | Declare `STORED`; expect catalog kind `s` |
| Virtual value | Unsupported; migration should reject or skip by policy | Declare `VIRTUAL`; expect catalog kind `v` |
| Kind omitted | Avoid in cross-major migrations | Defaults to virtual; test this only when intentional |

Run the real migration separately for each supported major. Do not infer PostgreSQL 17 compatibility from a passing PostgreSQL 18 job. The same unqualified `GENERATED ALWAYS AS (...)` text has a different availability and default-kind story at that boundary.

Virtual columns have stricter expression rules in PostgreSQL 18: they cannot use user-defined functions or types, including indirect use through operators or casts. Stored columns do not have that particular restriction. If a migration changes kind, the test must apply the DDL on a clean database rather than checking only runtime values on an already-migrated schema.

## 5. Run the proof in a disposable database

Save the harness in the repository, for example as `tests/postgres_generated_column_proof.py`, and run it through a one-shot PGSandbox session:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint "generated column proof" \
  --ttl-minutes 30 \
  --owner "agent-pr-482" \
  --label "suite=generated-columns" \
  --cleanup always \
  --result-format json \
  --env-var PGSANDBOX_DATABASE_URL \
  -- python tests/postgres_generated_column_proof.py
```

Repeat with every supported major or profile. `with-database` creates a tracked database and scoped login role, injects the credential into the child process, captures bounded credential-redacted output, and applies the selected cleanup policy. The [one-shot integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) documents the full session result contract, and the [PGSandbox MCP tool reference](/docs/mcp-tools/) covers the underlying lifecycle boundary.

Do not print `PGSANDBOX_DATABASE_URL` in CI logs or PR comments. Record the PostgreSQL major or profile, migration identifier, assertion summary, child exit status, and cleanup result. Keep the raw credential inside the child environment.

### Interpret each failed boundary

| Failed assertion | Likely problem |
| --- | --- |
| Expression or kind differs | Migration drift, implicit PostgreSQL 18 virtual default, or wrong schema selected |
| Explicit value does not return `428C9` | Test targeted a normal/identity column or used the wrong insert path |
| Insert value is wrong | Expression, type coercion, null behavior, or rounding differs |
| Update does not recompute | Column is not generated or dependency is missing from the expression |
| `AFTER` audit differs | Trigger ordering or stored/virtual assumption is wrong |
| One server major rejects DDL | Unsupported kind, function/type restriction, or version-specific syntax |
| Final state has extra rows | Failed case leaked state or fixtures were not reset |

Keep declaration failure separate from behavior failure. If the migrated column is wrong, stop before reporting the runtime assertions; otherwise the test produces noise against a schema it did not intend to validate.

## PR-ready generated-column proof

Keep the review note short enough to verify:

```text
Generated-column proof
- target: PostgreSQL 18, PGSandbox managed-local profile
- migration: applied the repository's real upgrade path
- declaration: net_amount, stored (`attgenerated = s`), expected expression
- write protection: explicit value rejected with SQLSTATE 428C9
- DEFAULT control: accepted and computed normally
- insert/update: exact generated values matched
- trigger boundary: BEFORE base rewrite reflected; AFTER saw stored value
- final state: exact ordered row set
- child exit: 0
- cleanup: sandbox deleted
```

For a multi-major project, add one line per major and show the generated kind each installed. Link the migration and test file. Do not attach database URLs or unbounded query output; the [bounded SQL evidence guide](/blog/postgres-run-sql-bounded-results/) explains how to keep agent-generated proof compact.

## PostgreSQL generated column testing FAQ

### How do I check whether a PostgreSQL column is generated?

Query `information_schema.columns` and inspect `is_generated` plus `generation_expression`. On PostgreSQL 18, query `pg_attribute.attgenerated` when the test must distinguish stored (`s`) from virtual (`v`) generation.

### Can an INSERT provide a value for a generated column?

It cannot provide an explicit value. PostgreSQL permits the `DEFAULT` keyword, which requests the normal generated-column behavior. An explicit value should fail with SQLSTATE `428C9` (`generated_always`).

### When does PostgreSQL recompute a generated column?

A stored generated column is computed when a row is inserted or updated. A virtual generated column is computed when it is read. Both derive from the declared base-column expression; callers cannot override the result.

### Can a BEFORE trigger read a generated column?

Do not read the new generated value in a `BEFORE` trigger. PostgreSQL computes stored generated columns after `BEFORE` triggers, and virtual generated columns are not computed while triggers fire. A base-column change made by a `BEFORE` trigger is reflected in the later generated result.

### Are virtual generated columns available before PostgreSQL 18?

No. PostgreSQL 18 introduced virtual generated columns and made virtual the default when the kind is omitted. Cross-version migrations that require stored behavior should specify `STORED` and test each supported major.

### Why use a disposable database for generated-column tests?

The proof applies real migrations, deliberately sends a forbidden direct write, may install test triggers, and should run across multiple PostgreSQL majors. A disposable [Postgres database sandbox](/blog/what-is-database-sandbox/) keeps those checks task-scoped and returns an explicit cleanup result.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "How to Test PostgreSQL Generated Columns",
      "description": "Prove the installed generated-column declaration, direct-write rejection, derivation, trigger timing, version behavior, final state, and cleanup.",
      "datePublished": "2026-08-05",
      "dateModified": "2026-08-05",
      "mainEntityOfPage": "https://pgsandbox.lvtd.dev/blog/test-postgresql-generated-columns/",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Inspect the declaration", "text": "Apply the real migration and assert the generation expression plus stored or virtual catalog kind."},
        {"@type": "HowToStep", "position": 2, "name": "Prove write protection", "text": "Send an explicit generated value, assert SQLSTATE 428C9, and keep DEFAULT as the successful control."},
        {"@type": "HowToStep", "position": 3, "name": "Prove derivation", "text": "Insert and update fixed base values, then assert exact generated results and final ordered rows."},
        {"@type": "HowToStep", "position": 4, "name": "Test the execution boundary", "text": "Prove BEFORE-trigger base changes feed stored generation and AFTER triggers observe the final stored value."},
        {"@type": "HowToStep", "position": 5, "name": "Test supported versions", "text": "Run the real migration on every supported PostgreSQL major and distinguish PostgreSQL 18 virtual behavior."},
        {"@type": "HowToStep", "position": 6, "name": "Clean up", "text": "Run the harness in a disposable PGSandbox database and verify the structured cleanup result."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do I check whether a PostgreSQL column is generated?", "acceptedAnswer": {"@type": "Answer", "text": "Query information_schema.columns for is_generated and generation_expression. On PostgreSQL 18, inspect pg_attribute.attgenerated to distinguish stored (s) from virtual (v) generation."}},
        {"@type": "Question", "name": "Can an INSERT provide a value for a generated column?", "acceptedAnswer": {"@type": "Answer", "text": "An INSERT cannot provide an explicit value for a generated column. PostgreSQL permits DEFAULT, which requests normal computation. An explicit value should fail with SQLSTATE 428C9."}},
        {"@type": "Question", "name": "Can a BEFORE trigger read a generated column?", "acceptedAnswer": {"@type": "Answer", "text": "Do not read the new generated value in a BEFORE trigger. Stored values are computed after BEFORE triggers, while virtual generated values are not computed while triggers fire."}},
        {"@type": "Question", "name": "Are virtual generated columns available before PostgreSQL 18?", "acceptedAnswer": {"@type": "Answer", "text": "No. PostgreSQL 18 introduced virtual generated columns and made virtual the default when VIRTUAL or STORED is omitted."}}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test PostgreSQL Generated Columns", "item": "https://pgsandbox.lvtd.dev/blog/test-postgresql-generated-columns/"}
      ]
    }
  ]
}
</script>
