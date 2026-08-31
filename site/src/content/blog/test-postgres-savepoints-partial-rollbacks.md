---
title: "How to Test Postgres Savepoints and Partial Rollbacks"
excerpt: "Force a statement error, roll back only the failed unit, preserve earlier work, continue the transaction, and verify the final state in disposable Postgres."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-27"
updatedAt: "2026-07-27T06:00:00Z"
tags: ["Postgres", "savepoints", "partial rollback", "transaction testing", "coding agents"]
category: "Engineering"
metaTitle: "Test Postgres Savepoints and Partial Rollbacks"
metaDescription: "Test Postgres savepoints by forcing SQLSTATE 23505, proving 25P02, rolling back the failed unit, and preserving the outer transaction."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/test-postgres-savepoints-partial-rollbacks/"
heroImageUrl: ""
featured: false
sortOrder: 149
---
Test a Postgres savepoint by doing useful work before it, creating provisional work after it, forcing a real statement error, and proving that `ROLLBACK TO SAVEPOINT` removes only the provisional unit. Then continue the same outer transaction, commit it, and verify the final rows from a new connection.

That sequence matters. A test that runs `SAVEPOINT` followed immediately by `ROLLBACK TO SAVEPOINT` proves the syntax, but it does not prove error recovery. A useful integration test should observe the failed transaction state, recover through the named savepoint, preserve earlier work, discard later work, and finish with an explicit final-state assertion.

This guide packages those checks into a **Partial Rollback Proof Contract**: boundary, failure, recovery, continuation, and cleanup. PGSandbox MCP supplies the disposable database lifecycle so the test can deliberately abort statements without changing shared development state.

## In this guide

- [Understand savepoint behavior](#what-a-postgres-savepoint-does)
- [Use the Partial Rollback Proof Contract](#the-partial-rollback-proof-contract)
- [Create a deterministic Psycopg harness](#1-create-a-deterministic-savepoint-test)
- [Run the proof in disposable Postgres](#2-run-the-proof-with-pgsandbox)
- [Verify the failed transaction state](#3-prove-the-transaction-requires-recovery)
- [Verify preserved and discarded work](#4-prove-the-partial-rollback-boundary)
- [Choose manual or driver-managed savepoints](#5-choose-the-right-savepoint-interface)
- [Avoid savepoint misuse](#6-keep-savepoints-bounded)
- [Record PR-ready evidence](#pr-ready-savepoint-proof)

## What a Postgres savepoint does

A PostgreSQL savepoint is a named position inside the current transaction. Commands before the savepoint remain part of the outer transaction. Commands after it belong to a recoverable inner unit until that savepoint is released or the transaction ends.

The current PostgreSQL [`SAVEPOINT` documentation](https://www.postgresql.org/docs/current/sql-savepoint.html) defines the command as a mark that can restore transaction state to the point at which the mark was created. Savepoints exist only inside a transaction block, and one transaction can contain several of them.

The three commands have different jobs:

| Command | Effect | Outer transaction |
| --- | --- | --- |
| `SAVEPOINT name` | Starts a recoverable inner unit | Remains active |
| `ROLLBACK TO SAVEPOINT name` | Discards work after the mark and starts a new subtransaction at that level | Remains active |
| `RELEASE SAVEPOINT name` | Keeps surviving work and merges it into the parent transaction context | Remains active |
| `ROLLBACK` | Discards the complete transaction | Ends |
| `COMMIT` | Makes the complete transaction durable | Ends |

PostgreSQL's [`ROLLBACK TO SAVEPOINT` reference](https://www.postgresql.org/docs/current/sql-rollback-to.html) adds two details that application tests often miss:

- The named savepoint remains valid after rollback and can be used again.
- Rolling back to it destroys every savepoint created after it.

`RELEASE SAVEPOINT` is not an error-recovery command. The current [`RELEASE SAVEPOINT` reference](https://www.postgresql.org/docs/current/sql-release-savepoint.html) says an aborted transaction cannot release a savepoint; it must first roll back to a valid savepoint or roll back the whole transaction.

### Savepoints recover one transaction, not other sessions

`ROLLBACK TO SAVEPOINT` affects only work in the current transaction after the named mark. It does not erase commits made by another connection. PostgreSQL implements explicit savepoints as subtransactions inside one top-level transaction, as described in the current [subtransaction internals documentation](https://www.postgresql.org/docs/current/subxacts.html).

That boundary is useful for batch imports, optional application steps, and nested service operations. It is not distributed rollback. An email, HTTP request, object-storage write, or payment sent before the database rollback remains external to PostgreSQL.

## The Partial Rollback Proof Contract

A reviewable savepoint test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Boundary | Which work must survive, and which work is provisional? | One insert before `SAVEPOINT`; another insert after it |
| Failure | Did PostgreSQL reject the intended statement? | SQLSTATE `23505` from a deliberate unique violation |
| Recovery | Was the transaction unusable before rollback and usable afterward? | SQLSTATE `25P02`, then a successful query after `ROLLBACK TO SAVEPOINT` |
| Continuation | Did the same outer transaction continue and commit? | A new insert after recovery plus final rows read from a new connection |
| Cleanup | Did the test close connections and remove the task database? | Structured PGSandbox session and deletion result |

This is the information gain over the common three-line savepoint example. It verifies the database state machine as well as the data: a real error aborts the current transaction context, rollback restores a usable point, earlier work survives, provisional work disappears, and later work can still commit.

The [PGSandbox MCP tool contract](/docs/mcp-tools/) exposes database lifecycle and bounded SQL operations. The savepoint harness itself belongs in one repository test process because all statements must use the same connection and outer transaction.

## 1. Create a deterministic savepoint test

The following Psycopg 3 script creates a table with a unique business key. It inserts one row before the savepoint, inserts a provisional row after it, then forces `23505` by reusing the first row's key.

Save it as `tests/postgres_savepoint_proof.py`:

```python
import json
import os

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]


def reset_fixture():
    with psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    ) as conn:
        conn.execute("DROP TABLE IF EXISTS savepoint_proof")
        conn.execute(
            """
            CREATE TABLE savepoint_proof (
                id integer PRIMARY KEY,
                external_ref text NOT NULL UNIQUE,
                state text NOT NULL
            )
            """
        )


def run_proof():
    with psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    ) as conn:
        conn.execute("BEGIN")
        conn.execute(
            """
            INSERT INTO savepoint_proof (id, external_ref, state)
            VALUES (1, 'accepted-1', 'before-savepoint')
            """
        )
        conn.execute("SAVEPOINT optional_item")
        conn.execute(
            """
            INSERT INTO savepoint_proof (id, external_ref, state)
            VALUES (2, 'provisional-2', 'after-savepoint')
            """
        )

        try:
            conn.execute(
                """
                INSERT INTO savepoint_proof (id, external_ref, state)
                VALUES (3, 'accepted-1', 'duplicate')
                """
            )
        except errors.UniqueViolation as exc:
            assert exc.sqlstate == "23505", exc.sqlstate
            failure_sqlstate = exc.sqlstate
        else:
            raise AssertionError("expected a unique violation")

        try:
            conn.execute("SELECT 1")
        except errors.InFailedSqlTransaction as exc:
            assert exc.sqlstate == "25P02", exc.sqlstate
            aborted_sqlstate = exc.sqlstate
        else:
            raise AssertionError("transaction was unexpectedly usable")

        conn.execute("ROLLBACK TO SAVEPOINT optional_item")

        visible_after_rollback = conn.execute(
            """
            SELECT id, external_ref, state
            FROM savepoint_proof
            ORDER BY id
            """
        ).fetchall()
        assert visible_after_rollback == [
            (1, "accepted-1", "before-savepoint")
        ], visible_after_rollback

        conn.execute(
            """
            INSERT INTO savepoint_proof (id, external_ref, state)
            VALUES (3, 'continued-3', 'after-recovery')
            """
        )
        conn.execute("RELEASE SAVEPOINT optional_item")
        conn.execute("COMMIT")

    with psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    ) as verifier:
        final_rows = verifier.execute(
            """
            SELECT id, external_ref, state
            FROM savepoint_proof
            ORDER BY id
            """
        ).fetchall()

    expected = [
        (1, "accepted-1", "before-savepoint"),
        (3, "continued-3", "after-recovery"),
    ]
    assert final_rows == expected, final_rows

    return {
        "failureSqlstate": failure_sqlstate,
        "abortedSqlstate": aborted_sqlstate,
        "beforeSavepointPreserved": True,
        "provisionalRowRemoved": True,
        "continuedAfterRollback": True,
        "finalRows": [list(row) for row in final_rows],
    }


reset_fixture()
print(json.dumps(run_proof(), sort_keys=True))
```

The fixture chooses a unique violation because it is deterministic and easy to classify. PostgreSQL's current [error-code appendix](https://www.postgresql.org/docs/current/errcodes-appendix.html) maps `23505` to `unique_violation` and `25P02` to `in_failed_sql_transaction`. Applications should inspect those stable codes instead of matching localized message text.

## 2. Run the proof with PGSandbox

Run the harness as one bounded child process:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 10 \
  --cleanup always \
  --timeout-seconds 60 \
  --result-format json \
  -- uv run --with "psycopg[binary]" \
    python tests/postgres_savepoint_proof.py
```

PGSandbox creates a task database and scoped login role, injects `PGSANDBOX_DATABASE_URL` and standard connection variables, supervises the child, redacts generated credentials from captured output, and applies the chosen cleanup policy. The [one-shot disposable database guide](/blog/run-integration-tests-disposable-postgres-database/) explains the complete session result and failure states.

Use `--cleanup always` for unattended CI and coding-agent runs. Use `--cleanup on-success` only when a failed database will actually be inspected. A retained database still needs a short TTL as a backstop.

The [PGSandbox architecture](/docs/architecture/) separates lifecycle authority from task SQL. The harness receives the sandbox role, not the administration credential that created the database.

## 3. Prove the transaction requires recovery

After PostgreSQL rejects the duplicate insert, the harness deliberately sends `SELECT 1` before recovery. It expects `25P02`.

That negative assertion is important. Without it, the test could pass even if the driver silently opened a different transaction, used autocommit for the failing statement, or performed an automatic rollback wider than intended. The expected `25P02` proves the connection is still attached to the failed transaction context.

Psycopg's current [transaction management documentation](https://www.psycopg.org/psycopg3/docs/basic/transactions.html) describes the same rule: after a database operation fails, PostgreSQL refuses ordinary commands until the application rolls back. A full `conn.rollback()` would discard the row inserted before the savepoint. The precise recovery command is:

```sql
ROLLBACK TO SAVEPOINT optional_item;
```

Once that succeeds, the transaction is usable again. The harness proves reuse with a read and another insert before committing.

The [statement timeout and cancellation guide](/blog/test-postgres-statement-timeouts-query-cancellation/) uses the same state-machine check for SQLSTATE `57014`: classify the original failure, prove `25P02` inside an explicit transaction, then recover at the intended boundary.

## 4. Prove the partial rollback boundary

The final rows are the strongest assertion:

```text
(1, accepted-1, before-savepoint)
(3, continued-3, after-recovery)
```

Each missing or present row proves something different:

- Row 1 proves work before the savepoint survived.
- Missing row 2 proves rollback removed successful provisional work, not only the statement that raised.
- Missing duplicate row proves the failed insert did not leak data.
- Row 3 proves the same outer transaction continued after recovery.
- Reading those rows from a new connection proves the outer transaction committed.

Do not stop at `SELECT 1` after rollback. Connection reuse proves recovery, but it does not prove the data boundary. Do not stop at an in-transaction query either. A separate verifier proves durability after `COMMIT`.

This distinction complements the [serialization failure retry guide](/blog/test-postgres-serialization-failure-retries/). A savepoint can recover a local statement error when earlier decisions remain valid. SQLSTATE `40001` invalidates the complete transaction and requires replay from a fresh snapshot; rolling back to a savepoint is not a substitute for whole-transaction retry.

## 5. Choose the right savepoint interface

Manual SQL is useful in a proof because every transition is visible. Production application code may be safer with the driver's nested transaction API.

Psycopg implements a nested `Connection.transaction()` block with a savepoint when an outer transaction is already active:

```python
with conn.transaction():
    conn.execute(
        """
        INSERT INTO savepoint_proof (id, external_ref, state)
        VALUES (1, 'accepted-1', 'outer')
        """
    )

    try:
        with conn.transaction():
            conn.execute(
                """
                INSERT INTO savepoint_proof (id, external_ref, state)
                VALUES (2, 'accepted-1', 'inner')
                """
            )
    except errors.UniqueViolation as exc:
        assert exc.sqlstate == "23505"

    conn.execute(
        """
        INSERT INTO savepoint_proof (id, external_ref, state)
        VALUES (3, 'continued-3', 'outer')
        """
    )
```

The inner exception causes the nested context to roll back its savepoint. Catch the exception outside that inner block, not inside it, so the context manager sees the failure and performs recovery. The outer context can then continue.

Use the interface your application actually ships. If an ORM implements nested transactions with savepoints, test the ORM boundary rather than replacing it with raw SQL in the integration test. Keep SQLSTATE assertions at the driver edge so the recovery policy remains explicit.

## 6. Keep savepoints bounded

Savepoints are recovery boundaries, not free checkpoints to add around every query.

PostgreSQL's [subtransaction documentation](https://www.postgresql.org/docs/current/subxacts.html) says each backend caches up to 64 open subtransaction IDs in shared memory. Keeping more open increases transaction-management overhead because PostgreSQL needs additional `pg_subtrans` lookups. Release or roll back completed savepoints and keep top-level transactions short.

Also account for these edge cases:

- **Later savepoints disappear.** `ROLLBACK TO SAVEPOINT outer_name` destroys savepoints created after `outer_name`.
- **Repeated names shadow older marks.** PostgreSQL retains older same-name savepoints, but the newest unreleased one is addressed first.
- **Invalid names are classified.** A missing savepoint produces SQLSTATE `3B001` (`invalid_savepoint_specification`).
- **Cursor state is special.** PostgreSQL documents that cursor creation, movement, closure, and query failure do not all rewind like table changes. Test cursor workflows directly if they matter.
- **External effects do not roll back.** Keep webhooks, messages, and payments after commit, or coordinate them with an outbox and idempotency key.

Savepoints are a good fit when one optional unit can fail while the outer transaction's earlier decisions remain valid. Use a full rollback when the error invalidates the complete operation, when retry policy requires a fresh snapshot, or when the application cannot state which work is safe to preserve.

## Common savepoint test failures

### The test catches the error but never rolls back

The next command should produce `25P02`. If code continues without a rollback, the driver or framework may be hiding the transaction boundary.

### The test calls `conn.rollback()`

A connection-level rollback normally aborts the complete transaction. That does not prove partial recovery. Use the named savepoint or the driver's nested transaction context.

### The exception is caught inside a nested context

If the context manager never sees the exception, it may release rather than roll back the savepoint. Catch outside the inner transaction block.

### Only the failing statement is checked

Add successful provisional work before the forced error. Rolling back should remove that work too, which proves the boundary covers every command after the mark.

### The test uses a shared database

The test intentionally creates constraints, provisional rows, and an aborted transaction. Run it in a task-scoped [database sandbox](/blog/what-is-database-sandbox/) with a scoped role, TTL, and cleanup.

## PR-ready savepoint proof

Record a compact result with a transaction-recovery change:

```text
Postgres savepoint proof
- Target: PostgreSQL=<major>, sandbox=<safe database ID>
- Boundary: row 1 before savepoint; row 2 provisional
- Failure: SQLSTATE 23505 unique_violation
- Failed state: SQLSTATE 25P02 before recovery
- Recovery: ROLLBACK TO SAVEPOINT optional_item
- Continuation: row 3 inserted and outer COMMIT succeeded
- Final rows: ids=[1,3], provisional id 2 absent
- Session: status=<status>, child exit=<code>, elapsed=<duration>
- Cleanup: policy=always, deleted=<yes/no>, error=<stable code or none>
```

That result shows the transaction boundary, original error, failed state, recovery command, committed invariant, and cleanup without exposing database credentials.

Install PGSandbox from the [setup guide](/docs/install/), then run the proof through your repository's real transaction abstraction.

## Frequently asked questions

### Does rollback to a savepoint end the transaction?

No. `ROLLBACK TO SAVEPOINT` discards commands after the named mark and starts a new subtransaction at the same level. The outer transaction remains active and can continue to execute SQL before it eventually commits or rolls back.

### Can a Postgres transaction continue after a statement error?

Yes, if the application established a valid savepoint before the error and rolls back to it. Until that recovery, ordinary commands in the failed transaction return SQLSTATE `25P02`. Without a savepoint, the application normally must roll back the complete transaction.

### Does rollback to a savepoint affect other database sessions?

No. A savepoint belongs to one transaction on one connection. Rolling back to it removes changes made in that transaction after the mark; it does not undo commits from other sessions.

### What is the difference between rollback and rollback to savepoint?

`ROLLBACK` aborts the complete transaction. `ROLLBACK TO SAVEPOINT name` preserves work before the named mark, discards work after it, and leaves the outer transaction active.

### Should every database operation use a savepoint?

No. Savepoints add transaction-management work and can obscure incorrect error handling. Use them around a clearly defined optional or nested unit whose failure does not invalidate earlier decisions in the outer transaction.

### Can a savepoint recover a serialization failure?

Do not use a savepoint as a general recovery path for SQLSTATE `40001`. PostgreSQL's documented serialization policy requires retrying the complete transaction from the beginning because its reads and decisions came from an invalidated snapshot.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Test Postgres savepoints and partial rollbacks",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Define the savepoint boundary", "text": "Insert durable outer work, establish a named savepoint, and add successful provisional work after it."},
        {"@type": "HowToStep", "position": 2, "name": "Force a statement error", "text": "Trigger a deterministic unique violation and assert PostgreSQL SQLSTATE 23505."},
        {"@type": "HowToStep", "position": 3, "name": "Prove the failed transaction state", "text": "Run an ordinary statement before recovery and require SQLSTATE 25P02."},
        {"@type": "HowToStep", "position": 4, "name": "Roll back to the savepoint", "text": "Execute ROLLBACK TO SAVEPOINT, verify earlier work remains and provisional work is gone, then continue the same outer transaction."},
        {"@type": "HowToStep", "position": 5, "name": "Commit and verify final state", "text": "Commit the outer transaction and read final rows from a new connection."},
        {"@type": "HowToStep", "position": 6, "name": "Clean up the database", "text": "Run the proof with pgsandbox with-database using a bounded timeout, credential-safe result, TTL, and explicit cleanup policy."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "Does rollback to a savepoint end the transaction?", "acceptedAnswer": {"@type": "Answer", "text": "No. It discards commands after the named mark while leaving the outer transaction active."}},
        {"@type": "Question", "name": "Can a Postgres transaction continue after a statement error?", "acceptedAnswer": {"@type": "Answer", "text": "Yes, when a valid savepoint existed before the error and the application rolls back to it. Before recovery, ordinary statements return SQLSTATE 25P02."}},
        {"@type": "Question", "name": "Does rollback to a savepoint affect other database sessions?", "acceptedAnswer": {"@type": "Answer", "text": "No. A savepoint belongs to one transaction and cannot undo commits from other sessions."}},
        {"@type": "Question", "name": "What is the difference between rollback and rollback to savepoint?", "acceptedAnswer": {"@type": "Answer", "text": "ROLLBACK aborts the complete transaction. ROLLBACK TO SAVEPOINT preserves work before the mark and keeps the outer transaction active."}},
        {"@type": "Question", "name": "Should every database operation use a savepoint?", "acceptedAnswer": {"@type": "Answer", "text": "No. Use savepoints around a defined optional or nested unit whose failure does not invalidate the outer transaction's earlier decisions."}},
        {"@type": "Question", "name": "Can a savepoint recover a serialization failure?", "acceptedAnswer": {"@type": "Answer", "text": "Do not use savepoints as a general recovery path for SQLSTATE 40001. Retry the complete transaction from the beginning with a fresh snapshot."}}
      ]
    }
  ]
}
</script>
