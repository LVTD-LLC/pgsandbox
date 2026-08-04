---
title: "How to Test Deferrable Constraints in PostgreSQL"
excerpt: "Prove immediate checks, temporary invalid states, SET CONSTRAINTS checkpoints, commit-time failures, final state, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-04"
updatedAt: "2026-08-04T06:00:00Z"
tags: ["Postgres", "deferrable constraints", "foreign keys", "integration testing", "coding agents"]
category: "Engineering"
metaTitle: "Test Deferrable Constraints in PostgreSQL"
metaDescription: "Test PostgreSQL deferrable constraints at statement, SET CONSTRAINTS, and commit boundaries with SQLSTATE checks and disposable cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-deferrable-constraints/"
heroImageUrl: ""
featured: false
sortOrder: 155
---
Test PostgreSQL deferrable constraints by proving behavior at three distinct boundaries: the statement that creates an invalid state, an explicit `SET CONSTRAINTS ... IMMEDIATE` checkpoint, and transaction commit. Inspect the installed constraint first, test both repair and failure paths, assert SQLSTATE `23503` or `23505`, then verify the final database state.

A test that only inserts rows in a convenient order does not exercise deferral. A test that expects an error without checking *when* it appears can also pass against the wrong constraint mode. The timing boundary is part of the schema contract.

This guide packages the proof into a five-part **Deferral Boundary Proof**: declaration, immediate control, temporary invalidity, validation boundary, and final state. PGSandbox MCP supplies a disposable database and scoped role on your configured PostgreSQL server, so a coding agent can run commit-time failure tests without leaving shared development data in a failed transaction.

*Published and last updated August 4, 2026.*

The complete proof has five steps:

1. Inspect whether the real migration installed a deferrable constraint and its initial mode.
2. Prove the initially immediate path rejects an invalid statement.
3. Defer the constraint, create a temporary violation, repair it, and force an early validation checkpoint.
4. Leave the same violation unrepaired and prove commit fails with the expected SQLSTATE.
5. Query the committed final state and delete the disposable database.

## In this guide

- [Understand deferrable constraints](#what-a-deferrable-constraint-changes)
- [Use the Deferral Boundary Proof](#the-deferral-boundary-proof)
- [Inspect the installed mode](#1-inspect-the-installed-constraint-mode)
- [Run a deterministic harness](#2-create-a-deterministic-deferrable-constraint-test)
- [Use a disposable database](#3-run-the-proof-with-pgsandbox)
- [Interpret each boundary](#4-interpret-the-constraint-timing-evidence)
- [Adapt the test to other constraints](#5-adapt-the-proof-to-unique-primary-key-and-exclusion-constraints)
- [Record review evidence](#pr-ready-deferrable-constraint-proof)
- [Answer common questions](#postgresql-deferrable-constraint-testing-faq)

## What a deferrable constraint changes

A deferrable constraint lets a transaction hold a temporarily invalid state that must become valid before the selected validation boundary. PostgreSQL can defer `UNIQUE`, `PRIMARY KEY`, `EXCLUDE`, and foreign-key constraints. `NOT NULL` and `CHECK` constraints are not deferrable.

PostgreSQL's current [`CREATE TABLE` reference](https://www.postgresql.org/docs/18/sql-createtable.html) defines three useful modes:

| Declaration | Initial behavior | Can `SET CONSTRAINTS` change it? |
| --- | --- | --- |
| `NOT DEFERRABLE` | Checked immediately | No |
| `DEFERRABLE INITIALLY IMMEDIATE` | Checked at the end of each statement | Yes |
| `DEFERRABLE INITIALLY DEFERRED` | Checked at transaction commit | Yes |

Deferral does not disable integrity. It moves the check. A transaction may insert a child before its parent, reorder values protected by a unique constraint, or make another temporary violation, but the final state still has to satisfy the constraint.

The [`SET CONSTRAINTS` documentation](https://www.postgresql.org/docs/18/sql-set-constraints.html) adds an important testing detail: switching a constraint from `DEFERRED` to `IMMEDIATE` is retroactive. PostgreSQL checks outstanding changes while executing `SET CONSTRAINTS`. That command is therefore a deliberate validation checkpoint, not merely session configuration.

### `NO ACTION` and `RESTRICT` do not have the same timing

For foreign keys, `NO ACTION` permits a deferrable check. The invalid relationship may exist temporarily when the constraint is deferred, provided the transaction repairs it before validation.

`RESTRICT` blocks the referenced-row change without allowing that check to be deferred. If a migration changes `NO ACTION` to `RESTRICT`, a commit-only happy-path test may miss the semantic change. Include the installed action and timing mode in the declaration evidence.

### Constraint timing is transaction-local

`SET CONSTRAINTS` affects the current transaction. A later transaction starts from the constraint's declared initial mode. Tests should set the mode inside the transaction they are exercising instead of relying on a previous test case or connection setup.

Use a stable, explicit constraint name. PostgreSQL constraint names do not have to be unique across an entire schema, and an unqualified name in `SET CONSTRAINTS` can match more than one constraint. Schema-qualify the name when ambiguity is possible.

## The Deferral Boundary Proof

A reviewable deferrable-constraint test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Declaration | What did the migration install? | Constraint type, definition, `is_deferrable`, and `initially_deferred` |
| Immediate control | Does the initial immediate mode reject a bad statement? | Expected SQLSTATE at statement execution |
| Temporary invalidity | Can deferred mode hold a violation long enough to repair it? | Invalid intermediate state followed by the missing parent or conflict repair |
| Validation boundary | Does failure appear at `SET CONSTRAINTS ... IMMEDIATE` and at commit when unrepaired? | Two separate failing transactions with the same expected SQLSTATE |
| Final state | Did only valid, intended data commit? | Exact ordered rows plus disposable database deletion |

The distinction between the two failure boundaries is the useful part. A forced `IMMEDIATE` checkpoint gives a precise assertion inside a larger transaction. A commit-time case proves the application path handles an exception raised while the transaction context exits, not only while an individual SQL statement runs.

This fits the broader [database migration testing workflow](/blog/database-migration-testing-agent-pr/): apply the real migration, inspect the installed object, exercise both sides of the behavior, and attach bounded evidence to the PR. It also complements the [savepoint and partial rollback guide](/blog/test-postgres-savepoints-partial-rollbacks/) when the application needs to recover from an expected failure without discarding unrelated outer work.

## 1. Inspect the installed constraint mode

Run the repository's real migration before the test. Recreating a similar foreign key in test setup proves the example DDL, not the change under review.

The portable inspection surface is `information_schema.table_constraints`:

```sql
SELECT
    constraint_schema,
    constraint_name,
    constraint_type,
    is_deferrable,
    initially_deferred
FROM information_schema.table_constraints
WHERE table_schema = 'public'
  AND table_name = 'deferral_tasks'
  AND constraint_name = 'deferral_tasks_project_fk';
```

PostgreSQL documents `is_deferrable` and `initially_deferred` in the current [`table_constraints` view](https://www.postgresql.org/docs/18/infoschema-table-constraints.html). Assert the exact result instead of checking only that some foreign key exists.

For PostgreSQL-specific detail, inspect `pg_constraint` and render the definition:

```sql
SELECT
    c.conname,
    c.contype,
    c.condeferrable,
    c.condeferred,
    c.convalidated,
    pg_get_constraintdef(c.oid, true) AS definition
FROM pg_constraint AS c
JOIN pg_class AS r ON r.oid = c.conrelid
JOIN pg_namespace AS n ON n.oid = r.relnamespace
WHERE n.nspname = 'public'
  AND r.relname = 'deferral_tasks'
  AND c.conname = 'deferral_tasks_project_fk';
```

The [`pg_constraint` catalog](https://www.postgresql.org/docs/18/catalog-pg-constraint.html) records whether a constraint is deferrable, deferred by default, enforced, and validated. Prefer semantic field assertions over a byte-for-byte comparison of `pg_get_constraintdef()` output.

## 2. Create a deterministic deferrable constraint test

The following Psycopg 3 harness uses a foreign key declared `DEFERRABLE INITIALLY IMMEDIATE`. It proves four paths independently: immediate rejection, deferred repair, forced early validation, and commit-time validation.

Save it as `tests/postgres_deferrable_constraint_proof.py`:

```python
import json
import os

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]
CONSTRAINT = "public.deferral_tasks_project_fk"


def connect():
    return psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    )


def reset_fixture(conn):
    conn.execute("DROP TABLE IF EXISTS deferral_tasks")
    conn.execute("DROP TABLE IF EXISTS deferral_projects")
    conn.execute(
        """
        CREATE TABLE deferral_projects (
            id integer PRIMARY KEY,
            name text NOT NULL
        )
        """
    )
    conn.execute(
        """
        CREATE TABLE deferral_tasks (
            id integer PRIMARY KEY,
            project_id integer NOT NULL,
            title text NOT NULL,
            CONSTRAINT deferral_tasks_project_fk
                FOREIGN KEY (project_id)
                REFERENCES deferral_projects (id)
                DEFERRABLE INITIALLY IMMEDIATE
        )
        """
    )


def constraint_declaration(conn):
    row = conn.execute(
        """
        SELECT
            constraint_type,
            is_deferrable,
            initially_deferred
        FROM information_schema.table_constraints
        WHERE table_schema = 'public'
          AND table_name = 'deferral_tasks'
          AND constraint_name = 'deferral_tasks_project_fk'
        """
    ).fetchone()
    return tuple(row)


def expect_foreign_key_violation(operation):
    try:
        operation()
    except errors.ForeignKeyViolation as exc:
        assert exc.sqlstate == "23503"
        return exc.sqlstate
    raise AssertionError("expected SQLSTATE 23503")


def immediate_failure(conn):
    def operation():
        with conn.transaction():
            conn.execute(
                """
                INSERT INTO deferral_tasks (id, project_id, title)
                VALUES (1, 100, 'immediate failure')
                """
            )

    return expect_foreign_key_violation(operation)


def deferred_repair(conn):
    with conn.transaction():
        conn.execute(f"SET CONSTRAINTS {CONSTRAINT} DEFERRED")
        conn.execute(
            """
            INSERT INTO deferral_tasks (id, project_id, title)
            VALUES (2, 200, 'child first')
            """
        )
        conn.execute(
            """
            INSERT INTO deferral_projects (id, name)
            VALUES (200, 'repaired before validation')
            """
        )
        conn.execute(f"SET CONSTRAINTS {CONSTRAINT} IMMEDIATE")


def checkpoint_failure(conn):
    def operation():
        with conn.transaction():
            conn.execute(f"SET CONSTRAINTS {CONSTRAINT} DEFERRED")
            conn.execute(
                """
                INSERT INTO deferral_tasks (id, project_id, title)
                VALUES (3, 300, 'fails at checkpoint')
                """
            )
            conn.execute(f"SET CONSTRAINTS {CONSTRAINT} IMMEDIATE")

    return expect_foreign_key_violation(operation)


def commit_failure(conn):
    def operation():
        with conn.transaction():
            conn.execute(f"SET CONSTRAINTS {CONSTRAINT} DEFERRED")
            conn.execute(
                """
                INSERT INTO deferral_tasks (id, project_id, title)
                VALUES (4, 400, 'fails at commit')
                """
            )

    return expect_foreign_key_violation(operation)


def final_state(conn):
    return conn.execute(
        """
        SELECT t.id, t.project_id, t.title, p.name
        FROM deferral_tasks AS t
        JOIN deferral_projects AS p ON p.id = t.project_id
        ORDER BY t.id
        """
    ).fetchall()


def main():
    with connect() as conn:
        reset_fixture(conn)
        declaration = constraint_declaration(conn)
        assert declaration == ("FOREIGN KEY", "YES", "NO")

        evidence = {
            "declaration": declaration,
            "immediateSqlstate": immediate_failure(conn),
        }

        deferred_repair(conn)
        evidence["checkpointSqlstate"] = checkpoint_failure(conn)
        evidence["commitSqlstate"] = commit_failure(conn)

        rows = [tuple(row) for row in final_state(conn)]
        assert rows == [
            (2, 200, "child first", "repaired before validation")
        ]
        evidence["finalRows"] = rows
        print(json.dumps(evidence, sort_keys=True))


if __name__ == "__main__":
    main()
```

The helper catches the error *outside* each transaction block. That detail matters for the commit case because the `ForeignKeyViolation` is raised while Psycopg exits the block and asks PostgreSQL to commit.

Psycopg's current [transaction documentation](https://www.psycopg.org/psycopg3/docs/basic/transactions.html) explains that failed database operations require rollback before the connection can continue. A transaction context performs that rollback when the exception leaves the block. Keep each expected failure in its own transaction so one aborted state cannot contaminate the next assertion.

PostgreSQL assigns SQLSTATE `23503` to `foreign_key_violation` and `23505` to `unique_violation` in its [error-code appendix](https://www.postgresql.org/docs/18/errcodes-appendix.html). Test the code, not localized message text.

## 3. Run the proof with PGSandbox

Install PGSandbox and confirm the configured PostgreSQL target by following the [setup guide](/docs/install/). PGSandbox does not install or host PostgreSQL. It creates a tracked database and scoped login role on the managed-local runtime or explicit profile you select.

Add Psycopg to the test environment, then run the proof as one bounded child process:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint "deferrable constraint proof" \
  --ttl-minutes 30 \
  --owner "agent-pr-482" \
  --label "suite=deferrable-constraints" \
  --result-format json \
  --env-var PGSANDBOX_DATABASE_URL \
  -- python tests/postgres_deferrable_constraint_proof.py
```

Use the version or profile that matches production. The [PGSandbox MCP tool contract](/docs/mcp-tools/) documents the same task database, scoped credential, TTL, and cleanup boundaries available to coding agents.

Do not print `PGSANDBOX_DATABASE_URL` in CI output or PR comments. Record the selected profile or PostgreSQL major, child exit status, structured assertion result, and cleanup status. Keep the credential-bearing URL inside the child process environment.

## 4. Interpret the constraint timing evidence

Each assertion rejects a different false positive:

| Assertion | What it proves | What failure usually means |
| --- | --- | --- |
| Declaration is `YES` / `NO` | Deferrable, initially immediate mode is installed | Migration omitted or changed the timing clause |
| Immediate insert returns `23503` | Default statement boundary is active | Test leaked deferred mode or migration is initially deferred |
| Child-first repair commits | Temporary invalid state is permitted and repaired | Constraint is not deferrable or repair targets the wrong key |
| Forced `IMMEDIATE` returns `23503` | Retroactive checkpoint validates outstanding changes | Wrong constraint selected or no violation was created |
| Transaction exit returns `23503` | Unrepaired state is rejected at commit | Driver code does not surface commit errors correctly |
| Only task `2` remains | Failed transactions rolled back completely | Autocommit or transaction scoping is wrong |

The final-state query is essential. A test can catch the right exception while accidentally committing setup rows from a previous transaction. Fixed identifiers and exact ordered rows make leakage visible.

If the application deliberately uses savepoints, add a case that wraps the expected validation failure in a nested transaction and proves the outer transaction can continue. Follow the [partial rollback proof](/blog/test-postgres-savepoints-partial-rollbacks/) rather than sending more commands to an aborted transaction.

## 5. Adapt the proof to unique, primary-key, and exclusion constraints

Keep the same five boundaries and change the temporary violation:

- **Deferrable unique constraint:** give two rows the same protected value, repair one before validation, then leave a duplicate unrepaired and expect SQLSTATE `23505`.
- **Deferrable primary key:** exercise the key rewrite or reorder that requires temporary duplication, then assert the complete final key set.
- **Deferrable exclusion constraint:** create a temporary overlap, repair the range before validation, and test the driver's mapped exclusion-violation error.
- **Deferrable foreign key:** insert child before parent, or update related keys in an order that is temporarily invalid, then assert SQLSTATE `23503` for the unrepaired path.

Do not use a deferrable constraint as an `INSERT ... ON CONFLICT` arbiter. PostgreSQL documents that deferrable constraints cannot serve that role. If the application depends on `ON CONFLICT`, add a migration test that proves the chosen non-deferrable unique index or constraint still supports the intended statement.

Also test the actual operation that motivated deferral. A generic child-before-parent example proves database semantics, but it does not prove an ORM reorder, graph import, bulk migration, or application transaction emits the correct sequence and handles commit errors.

## PR-ready deferrable constraint proof

Keep the review note short and reproducible:

```text
Deferrable constraint proof
- target: PostgreSQL 18, PGSandbox managed-local profile
- migration: applied the repository's real upgrade path
- declaration: FOREIGN KEY, DEFERRABLE, INITIALLY IMMEDIATE
- immediate control: SQLSTATE 23503 at INSERT
- repaired deferred path: child-first transaction committed
- forced checkpoint: SQLSTATE 23503 at SET CONSTRAINTS ... IMMEDIATE
- unrepaired path: SQLSTATE 23503 at COMMIT
- final state: exactly one repaired parent/task pair
- child exit: 0
- cleanup: sandbox deleted
```

Link the test file and migration in the PR. Do not attach database URLs, passwords, or unbounded query dumps. The [bounded SQL evidence guide](/blog/postgres-run-sql-bounded-results/) shows how to keep database proof compact enough for an agent and reviewer to inspect.

## PostgreSQL deferrable constraint testing FAQ

### How do I know whether a PostgreSQL constraint is deferrable?

Query `information_schema.table_constraints` and inspect `is_deferrable` plus `initially_deferred`. For PostgreSQL-specific details, query `pg_constraint.condeferrable` and `pg_constraint.condeferred`. Match the schema, table, and stable constraint name so the test cannot pass against a different object.

### When is a deferred constraint checked?

A deferred constraint is checked when the transaction commits or when `SET CONSTRAINTS ... IMMEDIATE` switches it back to immediate mode. The switch is retroactive, so outstanding violations cause the `SET CONSTRAINTS` statement itself to fail.

### Can PostgreSQL defer CHECK or NOT NULL constraints?

No. PostgreSQL currently allows deferral for `UNIQUE`, `PRIMARY KEY`, `EXCLUDE`, and foreign-key constraints. `CHECK` and `NOT NULL` constraints are not deferrable.

### Should a test use `INITIALLY DEFERRED` or call `SET CONSTRAINTS`?

Match the production declaration and application behavior. For `INITIALLY IMMEDIATE`, call `SET CONSTRAINTS ... DEFERRED` inside the tested transaction. Also include an immediate control case to prove the mode does not leak between transactions.

### Why does the constraint error appear when the transaction block exits?

An unrepaired deferred violation is discovered during commit. Drivers such as Psycopg issue that commit when a successful transaction context exits, so the exception is raised at the end of the block rather than at the earlier `INSERT` or `UPDATE`.

### Why use a disposable database for this test?

Deferral tests intentionally create invalid intermediate states, force transaction failures, and often rerun migrations. A disposable [Postgres database sandbox](/blog/what-is-database-sandbox/) keeps those destructive checks task-scoped and gives the agent an explicit cleanup result without granting it a shared application credential.
