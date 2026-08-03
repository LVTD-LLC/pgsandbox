---
title: "How to Test PostgreSQL Triggers in a Disposable Database"
excerpt: "Prove the installed trigger definition, firing paths, exact side effects, rollback behavior, SQLSTATE failures, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-03"
updatedAt: "2026-08-03T06:00:00Z"
tags: ["Postgres", "triggers", "integration testing", "migration testing", "coding agents"]
category: "Engineering"
metaTitle: "How to Test PostgreSQL Triggers"
metaDescription: "Test PostgreSQL triggers with catalog checks, exact side-effect assertions, rollback proof, SQLSTATE failures, and disposable database cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgresql-triggers/"
heroImageUrl: ""
featured: false
sortOrder: 154
---
Test PostgreSQL triggers by checking the installed definition, exercising every intended firing path, asserting the exact base-row and side-effect state, proving rollback removes both, and testing rejected input by SQLSTATE. Run the proof against the repository's real migration in a disposable database, then delete the database.

Checking that a trigger exists is not enough. A migration can install the wrong timing, event, condition, or function. A passing `INSERT` can also hide a missing audit row, an overly broad update, or a trigger error that leaves the transaction unusable.

This guide packages those checks into a five-part **Trigger Proof Contract**: declaration, firing matrix, effect oracle, atomicity, and teardown. PGSandbox MCP supplies a task-scoped database and restricted role on your configured PostgreSQL server. PostgreSQL catalog queries and application-level assertions supply the trigger proof.

*Published and last updated August 3, 2026.*

The complete proof has five steps:

1. Assert the exact non-internal trigger definition and enabled mode.
2. Exercise each intended event plus a negative path.
3. Compare exact target rows, side-effect rows, and an unrelated control row.
4. Recover from an expected trigger error, then prove outer rollback removes all successful trigger effects.
5. Record the pinned PostgreSQL target and delete the disposable database.

## In this guide

- [Understand PostgreSQL trigger behavior](#what-postgresql-triggers-do)
- [Use the Trigger Proof Contract](#the-trigger-proof-contract)
- [Inspect the installed trigger](#inspect-the-installed-trigger-definition)
- [Create the deterministic harness](#create-a-deterministic-trigger-test)
- [Run it with PGSandbox](#run-the-trigger-proof-in-a-disposable-database)
- [Interpret the evidence](#interpret-the-trigger-proof)
- [Cover advanced trigger shapes](#adapt-the-firing-matrix-to-your-trigger)
- [Record PR-ready evidence](#pr-ready-postgresql-trigger-proof)
- [Answer common trigger-testing questions](#postgresql-trigger-testing-faq)

## What PostgreSQL triggers do

A PostgreSQL trigger runs a function when a specified database event occurs. Data-change triggers can fire for `INSERT`, `UPDATE`, `DELETE`, or `TRUNCATE`. They can run once per affected row or once per statement, and their timing can be `BEFORE`, `AFTER`, or, for views, `INSTEAD OF`.

PostgreSQL's current [trigger behavior documentation](https://www.postgresql.org/docs/18/trigger-definition.html) states that a trigger runs in the same transaction as the statement that fired it. If either the statement or trigger raises an uncaught error, the effects of both are rolled back. That property makes rollback a useful test oracle, but only when the test observes both the target table and every table the trigger changes.

| Trigger shape | Firing boundary | What a test should prove |
| --- | --- | --- |
| `BEFORE ... FOR EACH ROW` | Before each matching row operation | Returned `NEW` value, skipped rows, validation errors |
| `AFTER ... FOR EACH ROW` | After each row operation, at statement end | Final row values and exact side effects |
| `BEFORE/AFTER ... FOR EACH STATEMENT` | Once per statement | Zero-row and multi-row statement behavior |
| `INSTEAD OF ... FOR EACH ROW` | Instead of a view row operation | Base-table changes and returned view row |
| Deferred constraint trigger | End of statement or transaction | `SET CONSTRAINTS` and commit-boundary behavior |

The [PostgreSQL `CREATE TRIGGER` reference](https://www.postgresql.org/docs/18/sql-createtrigger.html) makes an easy-to-miss distinction: a row trigger runs once per affected row, while a statement trigger runs once even when the statement affects zero rows. Do not reuse one firing matrix for both shapes.

### Trigger declarations and trigger behavior are separate evidence

Catalog inspection proves what PostgreSQL installed. It does not prove what the function does for your fixtures. Behavioral assertions prove outcomes, but they can pass against the wrong trigger if the fixture never reaches the missing branch.

You need both layers. The declaration check should identify the trigger by schema, table, and name. The behavior check should use fixed keys, an explicit firing matrix, exact counts, and a control row that should never change.

There is also a PGSandbox-specific boundary: current `describe_schema`, schema digests, and schema snapshots cover tables, columns, constraints, indexes, and extensions, but not user-authored triggers. Use the [schema snapshot workflow](/blog/postgres-schema-snapshots-agent-migration-reviews/) for the surrounding schema, then query `pg_trigger` for trigger-specific proof. Do not treat a clean snapshot diff as evidence that a trigger is equivalent.

## The Trigger Proof Contract

A reviewable PostgreSQL trigger test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Declaration | What trigger did the migration install? | Exact name, relation, function, enabled mode, timing, events, orientation, and rendered definition |
| Firing matrix | Which operations should and should not invoke it? | Matching event, negative path, multi-row or zero-row cases where relevant |
| Effect oracle | What exact state did it produce? | Target rows, side-effect rows, fixed fixture keys, explicit ordering, and a control row |
| Atomicity | Do trigger and statement effects share one transaction boundary? | In-transaction state, expected SQLSTATE, savepoint recovery, and final rollback state |
| Teardown | Did the test leave a bounded, disposable result? | Pinned profile or major, child exit status, cleanup result, and no retained task database |

Do not stop at the command tag. A result such as `INSERT 0 1` proves that the client statement completed. It does not show that an audit trigger wrote the expected row, that a denormalization trigger changed the right record, or that an unrelated row survived.

The same rule applies to agent-generated proof. Use bounded query results, but always add `ORDER BY` and exact expectations. The [bounded `run_sql` guide](/blog/postgres-run-sql-bounded-results/) explains why a row limit controls output size without making an unordered query deterministic.

## Inspect the installed trigger definition

Run the repository's real migration before the assertions. Recreating a look-alike trigger in test setup proves the example DDL, not the migration under review. The [database migration testing workflow](/blog/database-migration-testing-agent-pr/) shows how to apply the actual upgrade path inside a task database.

Then inspect the user-authored trigger by exact identity:

```sql
SELECT
    n.nspname AS schema_name,
    c.relname AS table_name,
    t.tgname AS trigger_name,
    t.tgenabled AS enabled_mode,
    p.proname AS function_name,
    pg_get_triggerdef(t.oid, true) AS definition
FROM pg_trigger AS t
JOIN pg_class AS c ON c.oid = t.tgrelid
JOIN pg_namespace AS n ON n.oid = c.relnamespace
JOIN pg_proc AS p ON p.oid = t.tgfoid
WHERE n.nspname = 'public'
  AND c.relname = 'trigger_accounts'
  AND t.tgname IN (
      'trigger_accounts_10_normalize',
      'trigger_accounts_90_audit'
  )
  AND NOT t.tgisinternal
ORDER BY t.tgname;
```

PostgreSQL documents `pg_trigger` as the native trigger catalog, including the target relation, function, enabled state, arguments, condition, and transition-table names. The [`pg_get_triggerdef()` system information function](https://www.postgresql.org/docs/18/functions-info.html#FUNCTIONS-INFO-CATALOG-TABLE) reconstructs a readable `CREATE TRIGGER` command.

Treat the reconstructed definition semantically. It is a decompiled representation, not the original migration text, so whitespace or equivalent syntax may differ. Assert stable parts such as the trigger name, timing, event list, table, orientation, and function name rather than comparing the complete string byte for byte.

The `tgenabled` value should match the expected replication mode. `O` means the trigger fires in the normal origin and local modes; `D` means disabled. Testing replica-only or always-enabled modes can require authority outside a normal application role, so place that case on a deliberately configured private profile instead of handing the test an admin connection.

## Create a deterministic trigger test

The harness below installs a compact example schema so the proof is runnable in isolation. In an application repository, replace `reset_fixture()` with the real migration command and keep the catalog and behavioral assertions.

The example uses a `BEFORE` trigger to normalize and validate email values and an `AFTER` trigger to append audit rows. It tests transformation, side effects, a control row, an expected SQLSTATE, savepoint recovery, and full rollback.

Save it as `tests/postgres_trigger_proof.py`:

```python
import json
import os

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]


def connect(*, autocommit=False):
    return psycopg.connect(
        DATABASE_URL,
        autocommit=autocommit,
        connect_timeout=5,
    )


def reset_fixture(conn):
    conn.execute("DROP TABLE IF EXISTS trigger_account_audit")
    conn.execute("DROP TABLE IF EXISTS trigger_accounts")
    conn.execute(
        """
        CREATE TABLE trigger_accounts (
            id integer PRIMARY KEY,
            email text NOT NULL,
            state text NOT NULL DEFAULT 'active'
        )
        """
    )
    conn.execute(
        """
        CREATE TABLE trigger_account_audit (
            account_id integer NOT NULL,
            operation text NOT NULL,
            old_email text,
            new_email text NOT NULL
        )
        """
    )
    conn.execute(
        """
        CREATE OR REPLACE FUNCTION normalize_trigger_account()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            IF btrim(NEW.email) !~ '^[^@[:space:]]+@[^@[:space:]]+$' THEN
                RAISE EXCEPTION USING
                    ERRCODE = '22023',
                    MESSAGE = 'invalid trigger account email';
            END IF;
            NEW.email := lower(btrim(NEW.email));
            RETURN NEW;
        END;
        $$
        """
    )
    conn.execute(
        """
        CREATE OR REPLACE FUNCTION audit_trigger_account()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            IF TG_OP = 'INSERT' THEN
                INSERT INTO trigger_account_audit
                    (account_id, operation, old_email, new_email)
                VALUES (NEW.id, TG_OP, NULL, NEW.email);
            ELSE
                INSERT INTO trigger_account_audit
                    (account_id, operation, old_email, new_email)
                VALUES (NEW.id, TG_OP, OLD.email, NEW.email);
            END IF;
            RETURN NULL;
        END;
        $$
        """
    )
    conn.execute(
        """
        CREATE TRIGGER trigger_accounts_10_normalize
        BEFORE INSERT OR UPDATE ON trigger_accounts
        FOR EACH ROW EXECUTE FUNCTION normalize_trigger_account()
        """
    )
    conn.execute(
        """
        CREATE TRIGGER trigger_accounts_90_audit
        AFTER INSERT OR UPDATE ON trigger_accounts
        FOR EACH ROW EXECUTE FUNCTION audit_trigger_account()
        """
    )


def trigger_definitions(conn):
    return conn.execute(
        """
        SELECT
            t.tgname,
            t.tgenabled,
            p.proname,
            pg_get_triggerdef(t.oid, true)
        FROM pg_trigger AS t
        JOIN pg_class AS c ON c.oid = t.tgrelid
        JOIN pg_namespace AS n ON n.oid = c.relnamespace
        JOIN pg_proc AS p ON p.oid = t.tgfoid
        WHERE n.nspname = 'public'
          AND c.relname = 'trigger_accounts'
          AND NOT t.tgisinternal
        ORDER BY t.tgname
        """
    ).fetchall()


def state(conn):
    accounts = conn.execute(
        "SELECT id, email, state FROM trigger_accounts ORDER BY id"
    ).fetchall()
    audit = conn.execute(
        """
        SELECT account_id, operation, old_email, new_email
        FROM trigger_account_audit
        ORDER BY account_id, operation, new_email
        """
    ).fetchall()
    return {
        "accounts": [tuple(row) for row in accounts],
        "audit": [tuple(row) for row in audit],
    }


def main():
    evidence = {}
    with connect(autocommit=True) as setup:
        reset_fixture(setup)
        definitions = trigger_definitions(setup)
        assert [row[0] for row in definitions] == [
            "trigger_accounts_10_normalize",
            "trigger_accounts_90_audit",
        ]
        assert all(row[1] == "O" for row in definitions)
        assert [row[2] for row in definitions] == [
            "normalize_trigger_account",
            "audit_trigger_account",
        ]
        assert "BEFORE INSERT OR UPDATE" in definitions[0][3]
        assert "AFTER INSERT OR UPDATE" in definitions[1][3]
        assert all("FOR EACH ROW" in row[3] for row in definitions)
        evidence["definitions"] = [tuple(row) for row in definitions]

    with connect() as conn:
        inserted = conn.execute(
            """
            INSERT INTO trigger_accounts (id, email)
            VALUES (1, '  OWNER@EXAMPLE.COM  ')
            RETURNING id, email, state
            """
        ).fetchone()
        assert tuple(inserted) == (1, "owner@example.com", "active")

        conn.execute(
            """
            INSERT INTO trigger_accounts (id, email, state)
            VALUES (2, 'control@example.com', 'control')
            """
        )
        before_failure = state(conn)

        try:
            with conn.transaction():
                conn.execute(
                    """
                    INSERT INTO trigger_accounts (id, email)
                    VALUES (3, 'not-an-email')
                    """
                )
        except errors.InvalidParameterValue as exc:
            assert exc.sqlstate == "22023"
        else:
            raise AssertionError("invalid email did not raise SQLSTATE 22023")

        assert state(conn) == before_failure

        updated = conn.execute(
            """
            UPDATE trigger_accounts
            SET email = '  NEW@EXAMPLE.COM  '
            WHERE id = 1
            RETURNING id, email, state
            """
        ).fetchone()
        assert tuple(updated) == (1, "new@example.com", "active")

        in_transaction = state(conn)
        assert in_transaction["accounts"] == [
            (1, "new@example.com", "active"),
            (2, "control@example.com", "control"),
        ]
        assert in_transaction["audit"] == [
            (1, "INSERT", None, "owner@example.com"),
            (1, "UPDATE", "owner@example.com", "new@example.com"),
            (2, "INSERT", None, "control@example.com"),
        ]
        evidence["in_transaction"] = in_transaction
        conn.rollback()

    with connect(autocommit=True) as verifier:
        after_rollback = state(verifier)
        assert after_rollback == {"accounts": [], "audit": []}
        evidence["after_rollback"] = after_rollback

    print(json.dumps(evidence, default=str, sort_keys=True))


if __name__ == "__main__":
    main()
```

The nested `conn.transaction()` context becomes a savepoint because the outer transaction is already active. Psycopg's current [transaction documentation](https://www.psycopg.org/psycopg3/docs/basic/transactions.html) documents that nested transaction contexts use savepoints. This lets the harness catch SQLSTATE `22023`, roll back only the rejected statement, and keep the outer transaction usable for later assertions.

PostgreSQL's [`RETURNING` documentation](https://www.postgresql.org/docs/18/dml-returning.html) notes that returned rows include changes made by triggers. That makes `RETURNING email` a direct oracle for the `BEFORE` transformation. The audit table still needs a separate query because it is an `AFTER` side effect.

The harness deliberately avoids timestamp and sequence assertions. Transaction rollback does not rewind sequence values, and wall-clock values make weak exact oracles. Fixed identifiers, explicit ordering, and exact tuples make failures easier to review.

## Run the trigger proof in a disposable database

Install Psycopg in the repository's normal development environment, then run the test through a one-shot PGSandbox session:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint trigger-proof \
  --ttl-minutes 30 \
  --cleanup always \
  --timeout-seconds 300 \
  --result-format json \
  -- uv run --with 'psycopg[binary]' python tests/postgres_trigger_proof.py
```

The CLI-only `pgsandbox with-database` command creates the task database and scoped login role, injects `DATABASE_URL`, `PGSANDBOX_DATABASE_URL`, and libpq connection variables into the child process, preserves bounded credential-redacted output, and applies the requested cleanup policy. The [disposable integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) documents the complete session result and cleanup contract.

PGSandbox uses an existing local or private PostgreSQL server. It does not install a new server for each test. Pin `--postgres-version` when the trigger must work across managed local majors, or use `--profile` when it depends on profile-specific extensions, settings, or network placement.

The sandbox role owns its task database but does not receive the lifecycle admin connection. That separation follows the [PGSandbox architecture](/docs/architecture/). A trigger proof should run with the same level of database authority the application or migration receives whenever possible.

For an MCP-led workflow, use the public [database lifecycle tools](/docs/mcp-tools/):

1. Call `create_database` with a short name hint, owner, labels, TTL, and pinned profile or PostgreSQL version.
2. Run the repository's migration and test command with `run_repo_command`.
3. Use `run_sql` for bounded catalog and final-state checks.
4. Call `delete_database` after capturing the proof packet.

Separate `run_sql` calls use separate connections, so they cannot coordinate an open transaction or savepoint. Keep the atomicity portion in one driver process, as the Psycopg harness does. Use `run_sql` before or after that process for declaration and final-state checks.

## Interpret the trigger proof

The JSON output should provide evidence for each contract field without exposing a database URL.

### Declaration

Two ordered catalog rows prove that the expected user-authored triggers exist and are enabled in normal mode. The rendered definitions establish the timing and event set. In an application test, also assert the exact function names and any important `WHEN`, `UPDATE OF`, or transition-table clauses.

Do not include internal triggers in the expected set. PostgreSQL uses internal triggers for features such as foreign-key enforcement. Filtering on `NOT tgisinternal` removes those generated objects. Exact schema, table, and trigger-name filters associate the remaining rows with the migration under review.

### Firing matrix

The insert and update cover both declared events. The invalid insert covers the negative validation branch. The harness uses two account rows so the test can prove that the update affects the target while the control remains unchanged.

If your trigger uses `UPDATE OF email`, add both `SET email = email` and an update to a different column. PostgreSQL documents that `UPDATE OF` is based on whether the column appears in the `SET` list, not whether its stored value changes. Use `WHEN (OLD.email IS DISTINCT FROM NEW.email)` when actual value change is the intended condition.

### Effect oracle

The target table proves the normalized row returned by the `BEFORE` trigger. The audit table proves the `AFTER` trigger's hidden side effects. Exact sorted rows make extra, missing, or misdirected writes visible.

An `AFTER` trigger sees the final row values and the effects of earlier triggers, which is why the expected audit email is normalized. PostgreSQL ignores an `AFTER` row trigger's return value, so the audit function returns `NULL` and proves its work through the audit table.

### Atomicity

The failed insert raises SQLSTATE `22023` inside a savepoint. After savepoint rollback, the prior account and audit rows remain visible inside the outer transaction and no rejected row appears. Rolling back the outer transaction then removes all target and audit rows.

This is stronger than catching an error on an autocommit connection. It proves the application can recover from the expected trigger rejection and that trigger side effects share the statement's transaction boundary.

### Teardown

The child result should identify whether provisioning, the command, or cleanup failed. `--cleanup always` requests database deletion after either a passing or failing child process. The TTL is a recovery backstop, not the normal cleanup mechanism.

If cleanup fails, keep the safe database ID and error category for a reviewed retry. Do not print or commit the injected connection URL.

## Adapt the firing matrix to your trigger

The example proves two row-level triggers. Other trigger shapes need different cases.

### Psycopg or pgTAP?

Use the repository's existing database test stack when it can express the declaration, behavior, and rollback assertions. The Psycopg harness above keeps application DML, SQLSTATE handling, and lifecycle evidence in one process. If your team already uses [pgTAP](https://pgtap.org/documentation.html), its `has_trigger()`, `trigger_is()`, query-result assertions, and `throws_ok()` can express the same contract in SQL. Run the pgTAP files inside the disposable database and retain the same firing matrix; extension availability is a profile prerequisite, not something the test should silently assume.

### How do I test a PostgreSQL trigger with pgTAP?

After the real migration and fixture setup, a compact pgTAP file can verify the installed trigger, transformed row, audit side effect, and rejection path:

```sql
BEGIN;
SELECT plan(5);

SELECT has_trigger(
    'public', 'trigger_accounts', 'trigger_accounts_10_normalize',
    'normalization trigger is installed'
);
SELECT trigger_is(
    'public', 'trigger_accounts', 'trigger_accounts_10_normalize',
    'public', 'normalize_trigger_account',
    'normalization trigger calls the expected function'
);

INSERT INTO trigger_accounts (id, email)
VALUES (1, '  OWNER@EXAMPLE.COM  ');

SELECT results_eq(
    $$SELECT id, email FROM trigger_accounts ORDER BY id$$,
    $$VALUES (1, 'owner@example.com'::text)$$,
    'BEFORE trigger normalizes the stored row'
);
SELECT results_eq(
    $$SELECT account_id, operation, new_email
      FROM trigger_account_audit
      ORDER BY account_id, operation, new_email$$,
    $$VALUES (1, 'INSERT'::text, 'owner@example.com'::text)$$,
    'AFTER trigger writes the expected audit row'
);
SELECT throws_ok(
    $$INSERT INTO trigger_accounts (id, email)
      VALUES (2, 'not-an-email')$$,
    '22023',
    'invalid trigger account email',
    'invalid input raises the expected SQLSTATE'
);

SELECT * FROM finish();
ROLLBACK;
```

Run the file with `pg_prove` or the repository's existing pgTAP runner inside `pgsandbox with-database`. Keep multi-row, zero-row, `WHEN`, and deferred cases in separate tests when those trigger shapes exist.

### Statement-level triggers

Run a statement that changes several rows, then one that matches zero rows. PostgreSQL statement triggers fire once for either statement, while row triggers fire once per affected row and do not fire for the zero-row case. Assert the statement-level audit count separately from the number of changed rows.

### `WHEN` conditions

Exercise one row where the condition is true and one where it is false. For an `AFTER` trigger, PostgreSQL evaluates `WHEN` after the row operation and uses it to decide whether to queue the trigger event. The false case should leave the side-effect table unchanged.

### Transition tables

An `AFTER` trigger can use `REFERENCING OLD TABLE` or `NEW TABLE` to inspect the complete row set changed by one statement. Test a multi-row statement and assert the entire transition-derived result, not one representative row. PostgreSQL limits transition relations to supported `AFTER` triggers on plain tables and does not allow them on constraint triggers.

### Deferred constraint triggers

Test both the statement boundary and the transaction boundary. A deferred constraint trigger may not raise until commit. Use `SET CONSTRAINTS ... IMMEDIATE` when you need to force a pending check before the final assertion, then test the real commit path separately.

### Recursive triggers and external effects

PostgreSQL does not impose a direct nesting limit on cascading triggers. If trigger SQL can modify its own table or another table that writes back, add a termination invariant and a bounded expected trace. Do not use `pg_trigger_depth()` as a substitute for a business rule without considering the wider trigger call stack.

A database rollback only proves transactional database effects. Notifications consumed by another process, HTTP calls through extensions, filesystem writes, and other external effects need their own test environment and cleanup. PGSandbox owns the PostgreSQL database lifecycle, not those external systems.

## PR-ready PostgreSQL trigger proof

Report the result as a compact proof packet:

```text
PostgreSQL trigger proof
- target: Postgres 18, disposable database <safe databaseId>
- migration: <exact repository migration command or revision>
- declaration: 2 expected non-internal triggers, enabled mode O
- firing matrix: INSERT, UPDATE, invalid INSERT
- effect oracle: normalized target row, 3 exact audit rows, control unchanged
- atomicity: SQLSTATE 22023 recovered by savepoint; outer rollback left 0 rows
- cleanup: deleted
```

Keep the database ID and structured result. Omit the admin URL, sandbox URL, and password. If the proof comes from an agent, include the exact direct child command and repository revision so a reviewer can rerun it.

Catalog state, behavioral state, and lifecycle state answer different questions. A publishable trigger proof contains all three.

## PostgreSQL trigger testing FAQ

### How can I tell whether a PostgreSQL trigger executed?

Assert an observable effect that only the trigger can produce, such as a normalized `RETURNING` value or an exact audit row. Catalog inspection proves that a trigger is installed, not that it fired for a particular statement. Avoid relying on logs or command tags as the only evidence.

### How do I list triggers on a PostgreSQL table?

Query `information_schema.triggers` for portable event, timing, and row-or-statement fields. For PostgreSQL-specific details, query `pg_trigger`, join it to `pg_class`, `pg_namespace`, and `pg_proc`, exclude `tgisinternal`, and render the definition with `pg_get_triggerdef()`.

### Are PostgreSQL triggers transactional?

Yes. PostgreSQL runs a trigger in the same transaction as the statement that fired it, so an uncaught error rolls back both statement and trigger effects. This guarantee covers database changes. External effects performed through extensions or other services may not be reversible by database rollback.

### How should I test a trigger that raises an exception?

Assert the SQLSTATE instead of matching the full error text. If later assertions must run in the same outer transaction, execute the expected failure inside a savepoint and roll back to it. Otherwise PostgreSQL leaves the transaction in a failed state until the client rolls it back.

### Does a schema diff prove that a PostgreSQL trigger is unchanged?

Only if the diff tool explicitly includes trigger definitions. PGSandbox's current schema digest and snapshot output do not include triggers, so use a targeted `pg_trigger` query plus behavioral assertions. Keep the schema diff for the surrounding tables, columns, constraints, indexes, and extensions; inspect trigger functions separately with a catalog query or `pg_get_functiondef()`.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Article",
      "headline": "How to Test PostgreSQL Triggers in a Disposable Database",
      "description": "Prove the installed trigger definition, firing paths, exact side effects, rollback behavior, SQLSTATE failures, and disposable cleanup.",
      "datePublished": "2026-08-03",
      "dateModified": "2026-08-03",
      "author": {"@type": "Organization", "name": "PGSandbox Team"},
      "publisher": {"@type": "Organization", "name": "PGSandbox MCP"},
      "mainEntityOfPage": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgresql-triggers/"
    },
    {
      "@type": "HowTo",
      "name": "How to Test PostgreSQL Triggers",
      "description": "Verify PostgreSQL trigger declarations, firing paths, exact effects, transaction behavior, and disposable cleanup.",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Inspect the declaration", "text": "Query pg_trigger by exact schema, table, and trigger name; exclude internal triggers and render each definition with pg_get_triggerdef()."},
        {"@type": "HowToStep", "position": 2, "name": "Exercise the firing matrix", "text": "Run every intended event plus a negative path through real INSERT, UPDATE, DELETE, or statement-level DML."},
        {"@type": "HowToStep", "position": 3, "name": "Assert exact effects", "text": "Compare ordered target rows, trigger side-effect rows, exact counts, and an unrelated control row."},
        {"@type": "HowToStep", "position": 4, "name": "Prove atomicity", "text": "Recover from the expected SQLSTATE inside a savepoint, then roll back the outer transaction and verify every successful transactional effect disappears."},
        {"@type": "HowToStep", "position": 5, "name": "Delete the sandbox", "text": "Record the pinned PostgreSQL target and structured command result, then remove the disposable database with TTL as a recovery backstop."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How can I tell whether a PostgreSQL trigger executed?", "acceptedAnswer": {"@type": "Answer", "text": "Assert an observable result that only the trigger can produce, such as a normalized RETURNING value or an exact audit row. Catalog inspection proves installation, not execution for a specific statement."}},
        {"@type": "Question", "name": "How do I list triggers on a PostgreSQL table?", "acceptedAnswer": {"@type": "Answer", "text": "Use information_schema.triggers for portable fields or query pg_trigger with pg_class, pg_namespace, and pg_proc for PostgreSQL-specific details. Exclude internal triggers and render definitions with pg_get_triggerdef()."}},
        {"@type": "Question", "name": "Are PostgreSQL triggers transactional?", "acceptedAnswer": {"@type": "Answer", "text": "Yes. PostgreSQL runs a trigger in the same transaction as the firing statement, so an uncaught error rolls back both statement and trigger database effects."}},
        {"@type": "Question", "name": "How should I test a trigger that raises an exception?", "acceptedAnswer": {"@type": "Answer", "text": "Assert the SQLSTATE. If later checks must remain in the outer transaction, run the expected failure inside a savepoint and roll back to it before continuing."}},
        {"@type": "Question", "name": "Does a schema diff prove that a PostgreSQL trigger is unchanged?", "acceptedAnswer": {"@type": "Answer", "text": "Only when the diff explicitly includes triggers. PGSandbox schema digests currently omit triggers, so combine a targeted pg_trigger query with behavioral assertions."}}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "Home", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test PostgreSQL Triggers", "item": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgresql-triggers/"}
      ]
    }
  ]
}
</script>
