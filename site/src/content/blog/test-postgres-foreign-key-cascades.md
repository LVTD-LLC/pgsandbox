---
title: "How to Test PostgreSQL Foreign Key Cascades"
excerpt: "Prove ON DELETE and ON UPDATE CASCADE behavior, rollback atomicity, unrelated-row survival, SQLSTATE 23503, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-31"
updatedAt: "2026-07-31T06:00:00Z"
tags: ["Postgres", "foreign keys", "cascade testing", "migration testing", "coding agents"]
category: "Engineering"
metaTitle: "Test PostgreSQL Foreign Key Cascades"
metaDescription: "Test PostgreSQL foreign key cascades with schema checks, positive and negative controls, rollback proof, SQLSTATE 23503, and cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-foreign-key-cascades/"
heroImageUrl: ""
featured: false
sortOrder: 152
---
Test PostgreSQL foreign key cascades by inspecting the installed constraint, mutating one known parent graph, and querying every affected table before and after rollback. A useful test proves the intended descendants change, unrelated rows survive, invalid references still fail with SQLSTATE `23503`, and the complete operation remains transactional.

Checking only that a parent `DELETE` succeeds is too weak. The database could be missing a child fixture, the migration could have installed `NO ACTION`, or an overly broad cleanup path could delete more than the intended graph. The final state is the contract.

This guide packages the checks into a **Cascade Proof Contract**: declaration, propagation, boundary, atomicity, and cleanup. PGSandbox MCP supplies a disposable database and scoped role on your configured PostgreSQL server, so a coding agent can run destructive referential-action tests without changing shared development data.

*Published and last updated July 31, 2026.*

The complete proof has five steps:

1. Inspect the installed foreign-key rules by stable constraint name.
2. Prove `ON UPDATE CASCADE` copies a changed parent key into child rows.
3. Prove `ON DELETE CASCADE` removes the complete targeted descendant graph.
4. Prove rollback restores the graph and unrelated rows never move.
5. Assert SQLSTATE `23503`, commit one final deletion, and remove the sandbox.

## In this guide

- [Understand PostgreSQL referential actions](#what-postgresql-foreign-key-cascades-do)
- [Use the Cascade Proof Contract](#the-cascade-proof-contract)
- [Create the deterministic harness](#1-create-a-deterministic-cascade-test)
- [Run it with PGSandbox](#2-run-the-proof-in-a-disposable-postgres-database)
- [Interpret each assertion](#3-interpret-the-cascade-proof)
- [Test migration changes](#4-test-a-cascade-migration-not-just-new-schema)
- [Check indexes and operational boundaries](#5-check-the-cost-and-boundary-of-cascades)
- [Record PR-ready evidence](#pr-ready-cascade-proof)

## What PostgreSQL foreign key cascades do

A PostgreSQL foreign key requires values in a referencing table to match a key in a referenced table. A referential action defines what happens to referencing rows when that parent key is deleted or updated.

The current PostgreSQL [foreign-key documentation](https://www.postgresql.org/docs/current/ddl-constraints.html#DDL-CONSTRAINTS-FK) defines `ON DELETE CASCADE` as automatic deletion of rows that reference the deleted parent. `ON UPDATE CASCADE` copies changed referenced-column values into the referencing rows. These are data actions. They are different from `DROP TABLE ... CASCADE`, which removes dependent database objects such as constraints.

| Referential action | Parent operation | Referencing-row result |
| --- | --- | --- |
| `CASCADE` | `DELETE` | Delete matching referencing rows |
| `CASCADE` | `UPDATE` of referenced key | Copy the new key value into matching referencing rows |
| `NO ACTION` | `DELETE` or key update | Allow the statement to proceed, then require a valid final constraint state; this is the default |
| `RESTRICT` | `DELETE` or key update | Reject the parent change without allowing the check to be deferred |
| `SET NULL` | `DELETE` or key update | Set configured referencing columns to `NULL`, if their other constraints allow it |
| `SET DEFAULT` | `DELETE` or key update | Set configured referencing columns to their defaults, which must still satisfy the foreign key |

A cascade can continue through more than one relationship. If `accounts` deletes `projects` and `projects` deletes `tasks`, deleting one account can remove both levels. That is useful only when the child rows share the parent's lifecycle. A foreign key should not cascade merely because deleting in application order is inconvenient.

### Foreign key CASCADE and DROP CASCADE are different

`ON DELETE CASCADE` is part of a foreign-key data policy. It deletes matching rows while keeping the tables and constraints.

`DROP ... CASCADE` is a schema dependency operation. PostgreSQL's [dependency-tracking documentation](https://www.postgresql.org/docs/current/ddl-depend.html) explains that dropping a referenced table with `CASCADE` can remove the foreign-key constraint that depends on it. It does not mean "exercise the foreign key's row-deletion rule."

Keep the distinction explicit in test names and PR evidence:

- `DELETE FROM accounts WHERE id = 1` tests a row-level referential action.
- `DROP TABLE accounts CASCADE` tests schema-object dependency removal.
- `TRUNCATE accounts CASCADE` has its own table-wide semantics and is not a substitute for testing a targeted `DELETE`.

## The Cascade Proof Contract

A reviewable cascade test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Declaration | Which rules did the migration actually install? | `information_schema.referential_constraints` rows matched by explicit constraint name |
| Propagation | Did the intended update and delete reach every expected child level? | Exact keys and row counts before, during, and after the operation |
| Boundary | Did unrelated parent graphs survive unchanged? | A control parent with its own project and task |
| Atomicity | Does rollback restore the whole graph, including cascaded changes? | Queries inside the transaction followed by the original rows after `ROLLBACK` |
| Cleanup | Did the test close connections and remove its task database? | Structured PGSandbox session and deletion result |

The boundary and atomicity checks are the information gain over a typical cascade example. A positive-only test can pass while deleting too much. A commit-only test cannot show that the cascade participates in the same transaction as the parent statement.

PostgreSQL's [`ROLLBACK` reference](https://www.postgresql.org/docs/current/sql-rollback.html) states that rollback discards all updates made by the transaction. That includes the child changes caused by referential actions. The test should observe both the temporary in-transaction state and the restored state from the same connection after rollback.

## 1. Create a deterministic cascade test

The following Psycopg 3 harness creates two independent account graphs. Account `1` is the target; account `2` is the negative control. Stable constraint names let the test verify the migration definition before trusting behavioral assertions.

Save this as `tests/postgres_cascade_proof.py`:

```python
import json
import os

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]


def connect():
    return psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    )


def reset_fixture():
    with connect() as conn:
        conn.execute("DROP TABLE IF EXISTS cascade_tasks")
        conn.execute("DROP TABLE IF EXISTS cascade_projects")
        conn.execute("DROP TABLE IF EXISTS cascade_accounts")
        conn.execute(
            """
            CREATE TABLE cascade_accounts (
                id integer PRIMARY KEY,
                name text NOT NULL
            )
            """
        )
        conn.execute(
            """
            CREATE TABLE cascade_projects (
                id integer PRIMARY KEY,
                account_id integer NOT NULL,
                name text NOT NULL,
                CONSTRAINT cascade_projects_account_fk
                    FOREIGN KEY (account_id)
                    REFERENCES cascade_accounts (id)
                    ON UPDATE CASCADE
                    ON DELETE CASCADE
            )
            """
        )
        conn.execute(
            """
            CREATE TABLE cascade_tasks (
                id integer PRIMARY KEY,
                project_id integer NOT NULL,
                title text NOT NULL,
                CONSTRAINT cascade_tasks_project_fk
                    FOREIGN KEY (project_id)
                    REFERENCES cascade_projects (id)
                    ON DELETE CASCADE
            )
            """
        )
        conn.execute(
            """
            CREATE INDEX cascade_projects_account_idx
            ON cascade_projects (account_id)
            """
        )
        conn.execute(
            """
            CREATE INDEX cascade_tasks_project_idx
            ON cascade_tasks (project_id)
            """
        )
        conn.execute(
            """
            INSERT INTO cascade_accounts (id, name)
            VALUES (1, 'target'), (2, 'control')
            """
        )
        conn.execute(
            """
            INSERT INTO cascade_projects (id, account_id, name)
            VALUES
                (10, 1, 'target-a'),
                (11, 1, 'target-b'),
                (20, 2, 'control')
            """
        )
        conn.execute(
            """
            INSERT INTO cascade_tasks (id, project_id, title)
            VALUES
                (100, 10, 'target-a-1'),
                (101, 10, 'target-a-2'),
                (110, 11, 'target-b-1'),
                (200, 20, 'control-1')
            """
        )


def fetch_rules(conn):
    rows = conn.execute(
        """
        SELECT constraint_name, update_rule, delete_rule
        FROM information_schema.referential_constraints
        WHERE constraint_schema = current_schema()
          AND constraint_name IN (
              'cascade_projects_account_fk',
              'cascade_tasks_project_fk'
          )
        ORDER BY constraint_name
        """
    ).fetchall()
    return [tuple(row) for row in rows]


def graph_counts(conn):
    return tuple(
        conn.execute(
            """
            SELECT
                (SELECT count(*) FROM cascade_accounts),
                (SELECT count(*) FROM cascade_projects),
                (SELECT count(*) FROM cascade_tasks)
            """
        ).fetchone()
    )


def prove_invalid_reference(conn):
    try:
        with conn.transaction():
            conn.execute(
                """
                INSERT INTO cascade_tasks (id, project_id, title)
                VALUES (999, 999, 'orphan')
                """
            )
    except errors.ForeignKeyViolation as error:
        assert error.sqlstate == "23503"
        return error.sqlstate
    raise AssertionError("invalid child reference unexpectedly succeeded")


def prove_update_rollback(conn):
    conn.execute("BEGIN")
    conn.execute(
        "UPDATE cascade_accounts SET id = 101 WHERE id = 1"
    )
    updated_keys = [
        row[0]
        for row in conn.execute(
            """
            SELECT account_id
            FROM cascade_projects
            WHERE id IN (10, 11)
            ORDER BY id
            """
        ).fetchall()
    ]
    assert updated_keys == [101, 101]
    assert conn.execute(
        "SELECT account_id FROM cascade_projects WHERE id = 20"
    ).fetchone()[0] == 2
    conn.execute("ROLLBACK")

    restored_keys = [
        row[0]
        for row in conn.execute(
            """
            SELECT account_id
            FROM cascade_projects
            WHERE id IN (10, 11)
            ORDER BY id
            """
        ).fetchall()
    ]
    assert restored_keys == [1, 1]
    return {"during_update": updated_keys, "after_rollback": restored_keys}


def prove_delete_rollback(conn):
    before = graph_counts(conn)
    assert before == (2, 3, 4)

    conn.execute("BEGIN")
    deleted_parent = conn.execute(
        """
        DELETE FROM cascade_accounts
        WHERE id = 1
        RETURNING id
        """
    ).fetchone()[0]
    assert deleted_parent == 1
    during = graph_counts(conn)
    assert during == (1, 1, 1)
    assert conn.execute(
        "SELECT title FROM cascade_tasks WHERE id = 200"
    ).fetchone()[0] == "control-1"
    conn.execute("ROLLBACK")

    after = graph_counts(conn)
    assert after == before
    return {"before": before, "during": during, "after": after}


def prove_delete_commit(conn):
    conn.execute("BEGIN")
    conn.execute("DELETE FROM cascade_accounts WHERE id = 1")
    conn.execute("COMMIT")

    final_counts = graph_counts(conn)
    assert final_counts == (1, 1, 1)
    remaining = {
        "account_ids": [
            row[0]
            for row in conn.execute(
                "SELECT id FROM cascade_accounts ORDER BY id"
            ).fetchall()
        ],
        "project_ids": [
            row[0]
            for row in conn.execute(
                "SELECT id FROM cascade_projects ORDER BY id"
            ).fetchall()
        ],
        "task_ids": [
            row[0]
            for row in conn.execute(
                "SELECT id FROM cascade_tasks ORDER BY id"
            ).fetchall()
        ],
    }
    assert remaining == {
        "account_ids": [2],
        "project_ids": [20],
        "task_ids": [200],
    }
    return {"counts": final_counts, "remaining": remaining}


def main():
    reset_fixture()

    with connect() as conn:
        rules = fetch_rules(conn)
        assert rules == [
            ("cascade_projects_account_fk", "CASCADE", "CASCADE"),
            ("cascade_tasks_project_fk", "NO ACTION", "CASCADE"),
        ]
        sqlstate = prove_invalid_reference(conn)
        update = prove_update_rollback(conn)
        delete_rollback = prove_delete_rollback(conn)
        delete_commit = prove_delete_commit(conn)

    print(
        json.dumps(
            {
                "rules": rules,
                "invalid_reference_sqlstate": sqlstate,
                "update": update,
                "delete_rollback": delete_rollback,
                "delete_commit": delete_commit,
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
```

The script uses exact fixture identifiers rather than generated IDs, sleeps, or probabilistic timing. Every failure points to one part of the contract: schema definition, update propagation, delete propagation, rollback, boundary, or integrity rejection.

The `information_schema.referential_constraints` view is the portable inspection surface. PostgreSQL's current [view documentation](https://www.postgresql.org/docs/current/infoschema-referential-constraints.html) exposes `update_rule` and `delete_rule` as named values including `CASCADE`, `RESTRICT`, and `NO ACTION`. Matching explicit constraint names prevents the test from silently inspecting a different relationship.

## 2. Run the proof in a disposable Postgres database

Install the test dependency in your project environment:

```bash
python -m pip install "psycopg[binary]>=3.2,<4"
```

Then run the proof through a one-shot PGSandbox session:

```bash
pgsandbox with-database \
  --label foreign-key-cascade-proof \
  --ttl-minutes 30 \
  --env-var PGSANDBOX_DATABASE_URL \
  -- python tests/postgres_cascade_proof.py
```

`with-database` creates a tracked database and scoped login role, injects the sandbox connection into the child process, captures bounded output, and applies the selected cleanup policy. The broader [disposable Postgres integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) explains that one-shot lifecycle and the evidence returned to an agent.

If your repository already has a migration runner, replace `reset_fixture()` with the real migration command and seed only the two compact graphs. The cascade proof should exercise the schema your application will ship, not a hand-written approximation.

The task role owns its sandbox database but does not receive cluster-wide admin authority. That separation follows the [per-sandbox Postgres role model](/blog/per-sandbox-postgres-roles-coding-agents/) and keeps deliberate destructive tests inside one disposable database.

## 3. Interpret the cascade proof

Each assertion catches a different regression.

### Declaration: inspect the schema before behavior

The expected rules are:

```text
cascade_projects_account_fk | CASCADE   | CASCADE
cascade_tasks_project_fk    | NO ACTION | CASCADE
```

The task relationship does not declare `ON UPDATE CASCADE`, so PostgreSQL reports the default `NO ACTION`. This is not a test omission. It proves that the harness distinguishes explicit behavior from defaults.

Schema inspection matters during migrations that drop and recreate constraints. A behavioral test with no matching child rows can pass even when the new constraint has the wrong action. The declaration check fails immediately.

PostgreSQL implements foreign-key enforcement with internal triggers, but application-owned triggers need a different catalog filter and firing matrix. The [PostgreSQL trigger testing guide](/blog/test-postgresql-triggers/) shows how to exclude `tgisinternal`, inspect rendered user-trigger definitions, and prove target rows, side effects, SQLSTATE failures, and rollback separately.

### Propagation: assert the complete graph

The update probe changes account key `1` to `101`. Both target projects must receive `101`; the control project must remain attached to account `2`. The test then rolls back and expects the original keys.

The delete probe begins with `(2 accounts, 3 projects, 4 tasks)`. Inside the transaction, deleting account `1` must produce `(1, 1, 1)`. That proves both direct project deletion and the second-level task cascade. After rollback, the original `(2, 3, 4)` graph must return.

Do not infer child deletion from the parent's `DELETE 1` command tag. That count describes rows directly deleted from the target table, not a complete audit of rows affected through referential actions. Query the child tables.

### Boundary: keep a negative control

The account `2` graph is deliberately similar to the target graph. It remains present during the rolled-back delete and after the committed delete:

```json
{
  "account_ids": [2],
  "project_ids": [20],
  "task_ids": [200]
}
```

This control catches an application helper that issues an unscoped delete, truncates a whole table, or uses the wrong tenant predicate. PostgreSQL can enforce a correctly declared foreign key while application SQL still selects the wrong parent.

### Atomicity: observe state before and after rollback

The rollback probes prove two states, not one:

1. Inside the transaction, the cascaded child changes are visible to that transaction.
2. After rollback, the parent and all affected children return to their original values.

Psycopg's current [transaction documentation](https://www.psycopg.org/psycopg3/docs/basic/transactions.html) explains that transaction contexts commit on normal exit and roll back on exceptions. This harness uses explicit `BEGIN` and `ROLLBACK` so the two observation points remain visible in the test.

### Integrity: assert SQLSTATE, not message text

The invalid task reference must fail with `23503`, PostgreSQL's `foreign_key_violation` condition. The PostgreSQL [error-code appendix](https://www.postgresql.org/docs/current/errcodes-appendix.html) recommends branching on SQLSTATE instead of localized message text.

That negative test proves the foreign key still protects ordinary inserts. A migration should not gain convenient cascades by accidentally dropping referential integrity.

## 4. Test a cascade migration, not just new schema

Changing an existing foreign key deserves two lanes:

1. **Existing-data lane:** clone or seed a representative pre-migration database, run the migration, inspect the installed action, and exercise target plus control graphs.
2. **Fresh-schema lane:** build a new database from the complete migration history and run the same proof.

The existing-data lane catches validation and migration-order problems. The fresh-schema lane catches drift between the current schema and the path a new environment follows. The [database migration testing workflow](/blog/database-migration-testing-agent-pr/) shows how to collect before/after schema evidence in a disposable database.

For a migration that replaces a constraint, give it an explicit name and make the transition reviewable:

```sql
ALTER TABLE cascade_projects
    DROP CONSTRAINT cascade_projects_account_fk,
    ADD CONSTRAINT cascade_projects_account_fk
        FOREIGN KEY (account_id)
        REFERENCES cascade_accounts (id)
        ON UPDATE CASCADE
        ON DELETE CASCADE;
```

Do not treat `NOT VALID` as "the cascade applies only to new rows." PostgreSQL's [`ALTER TABLE` documentation](https://www.postgresql.org/docs/current/sql-altertable.html) says `NOT VALID` defers the initial scan of old rows when adding a foreign key, while subsequent inserts and updates are still checked. If your migration uses that pattern, add a separate `VALIDATE CONSTRAINT` step and prove the final constraint is validated before calling the rollout complete.

## 5. Check the cost and boundary of cascades

Correct behavior is necessary, but a large cascade can still be operationally expensive.

PostgreSQL's foreign-key documentation notes that a primary or unique referenced key has an index, but declaring the foreign key does not automatically index the referencing columns. A parent delete or key update must find matching child rows. Index `cascade_projects.account_id` and `cascade_tasks.project_id` when the workload and table size justify it, then review the real plan and lock behavior on representative data.

Use cascades when the child has the same deletion lifecycle as the parent: line items owned by an order, tasks owned by a disposable project, or transient records that should not outlive their container. Prefer `RESTRICT` or `NO ACTION` when deletion needs an explicit archive, approval, transfer, or audit decision.

Also test application-level side effects separately. A database cascade changes rows. It does not automatically run an ORM callback, revoke an external credential, delete an object-storage file, or cancel a queued job. If those effects matter, the application workflow needs its own proof.

## Common PostgreSQL cascade testing mistakes

### Testing with an empty child table

A parent delete succeeds under both `CASCADE` and a missing relationship when no child references the parent. Seed at least two child levels and assert exact final identifiers.

### Checking only counts

Counts can match after the wrong rows were deleted. Pair counts with stable target and control IDs.

### Confusing DELETE CASCADE with TRUNCATE CASCADE

`TRUNCATE ... CASCADE` operates on whole tables that reference the truncated table. It is useful for some test resets, but it does not prove the targeted row behavior of `ON DELETE CASCADE`.

### Omitting rollback

A committed happy path shows the destination state. It does not prove that a later statement failure or an explicit abort restores the complete graph.

### Matching English errors

Constraint messages can change or be localized. Assert SQLSTATE `23503` and, when useful, the structured constraint name exposed by the driver.

### Running destructive proofs in shared development data

Even a correct cascade deliberately deletes rows. Run it in a fresh [database sandbox](/blog/what-is-database-sandbox/) with a bounded TTL and explicit cleanup.

## PR-ready cascade proof

A compact PR note should record:

```text
PostgreSQL foreign-key cascade proof
- database: disposable sandbox (databaseId recorded outside the public PR)
- schema: explicit constraints inspected through information_schema
- update: account 1 -> 101 propagated to projects 10 and 11
- delete: target graph changed from 2/3/4 to 1/1/1
- boundary: account 2, project 20, and task 200 survived
- rollback: update and delete graphs returned to original state
- integrity: invalid task reference returned SQLSTATE 23503
- commit: only control graph remained
- cleanup: sandbox removed by the one-shot session
```

Do not paste the sandbox connection string. Record the database ID, PostgreSQL version, migration revision, command exit status, assertion summary, and cleanup result.

## Frequently asked questions

### How do you test ON DELETE CASCADE in PostgreSQL?

Seed a parent, at least one child level, and a separate control graph. Delete the target parent inside a transaction, query exact child identifiers before rollback, then confirm rollback restores every row. Commit a second run and verify only the control graph remains.

### How do you check whether a PostgreSQL foreign key uses CASCADE?

Query `information_schema.referential_constraints` and inspect `delete_rule` and `update_rule` for a stable constraint name. PostgreSQL also exposes internal action codes in `pg_constraint`, but the information-schema view gives readable values and is the better default for application tests.

### Does ON DELETE CASCADE roll back in PostgreSQL?

Yes. Cascaded row changes participate in the transaction that issued the parent delete. If the transaction rolls back, PostgreSQL discards the parent deletion and the child deletions caused by the foreign-key action.

### What SQLSTATE identifies a PostgreSQL foreign key violation?

SQLSTATE `23503` is `foreign_key_violation`. Tests should use that stable code rather than matching English error text.

### Does PostgreSQL create an index for a foreign key?

PostgreSQL indexes the referenced primary or unique key, but it does not automatically create an index on the referencing columns. Parent deletes and referenced-key updates may need to scan the child table, so evaluate and usually index those columns for nontrivial workloads.

### Should every foreign key use ON DELETE CASCADE?

No. Use it when the child should never outlive the parent and deletion requires no separate business decision. Use `RESTRICT` or `NO ACTION` when child records need transfer, archival, approval, or independent retention.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "How to Test PostgreSQL Foreign Key Cascades",
      "description": "Prove foreign-key declarations, update and delete propagation, unrelated-row survival, rollback atomicity, SQLSTATE 23503, and cleanup.",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Inspect installed constraints", "text": "Query information_schema by stable constraint name and assert the update and delete rules."},
        {"@type": "HowToStep", "position": 2, "name": "Prove update propagation", "text": "Update one referenced key and verify only its child foreign-key values change."},
        {"@type": "HowToStep", "position": 3, "name": "Prove delete propagation", "text": "Delete one parent and query every expected descendant table plus an unrelated control graph."},
        {"@type": "HowToStep", "position": 4, "name": "Prove rollback atomicity", "text": "Observe cascaded changes inside the transaction, roll back, and verify the complete graph returns."},
        {"@type": "HowToStep", "position": 5, "name": "Verify integrity and clean up", "text": "Assert SQLSTATE 23503 for an invalid reference, commit the final proof, and remove the disposable database."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do you test ON DELETE CASCADE in PostgreSQL?", "acceptedAnswer": {"@type": "Answer", "text": "Seed a target parent graph and an unrelated control graph, delete the target inside a transaction, query exact child identifiers, prove rollback restores the graph, then commit a second run and verify only the control graph remains."}},
        {"@type": "Question", "name": "How do you check whether a PostgreSQL foreign key uses CASCADE?", "acceptedAnswer": {"@type": "Answer", "text": "Query information_schema.referential_constraints and inspect delete_rule and update_rule for a stable constraint name."}},
        {"@type": "Question", "name": "Does ON DELETE CASCADE roll back in PostgreSQL?", "acceptedAnswer": {"@type": "Answer", "text": "Yes. Cascaded child changes participate in the same transaction as the parent deletion, so rollback restores both parent and child rows."}},
        {"@type": "Question", "name": "What SQLSTATE identifies a PostgreSQL foreign key violation?", "acceptedAnswer": {"@type": "Answer", "text": "SQLSTATE 23503 identifies foreign_key_violation. Tests should assert that code instead of localized error text."}},
        {"@type": "Question", "name": "Does PostgreSQL create an index for a foreign key?", "acceptedAnswer": {"@type": "Answer", "text": "PostgreSQL indexes the referenced primary or unique key but does not automatically index the referencing columns."}},
        {"@type": "Question", "name": "Should every foreign key use ON DELETE CASCADE?", "acceptedAnswer": {"@type": "Answer", "text": "No. Use it only when child rows share the parent's lifecycle; prefer restrictive actions when deletion requires transfer, archival, approval, or independent retention."}}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test PostgreSQL Foreign Key Cascades", "item": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-foreign-key-cascades/"}
      ]
    }
  ]
}
</script>
