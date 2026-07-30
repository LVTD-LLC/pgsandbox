---
title: "How to Test Postgres Advisory Locks Safely"
excerpt: "Prove advisory-lock contention, session and transaction lifetime, catalog visibility, release, and cleanup with two real Postgres connections."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-29"
updatedAt: "2026-07-29T06:00:00Z"
tags: ["Postgres", "advisory locks", "concurrency testing", "integration testing", "coding agents"]
category: "Engineering"
metaTitle: "Test Postgres Advisory Locks Safely"
metaDescription: "Test Postgres advisory locks with two connections, nonblocking contention, pg_locks evidence, rollback checks, disconnect release, and cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-advisory-locks/"
heroImageUrl: ""
featured: false
sortOrder: 150
---
Test Postgres advisory locks with two independent connections, a fixed application key, and nonblocking `pg_try_advisory_*` calls. The proof should show one connection acquiring the lock, the other being denied, the lock appearing in `pg_locks`, and the resource becoming available at the exact boundary promised by the lock type.

That last check catches lock-lifetime bugs. A session-level advisory lock survives transaction rollback and remains held until it is explicitly unlocked or the connection ends. A transaction-level advisory lock is released automatically when its transaction commits or rolls back. A test that asserts only the first successful acquisition cannot distinguish those lifetimes.

This guide packages the checks into an **Advisory Lock Proof Contract**: identity, contention, visibility, lifetime, and cleanup. PGSandbox MCP creates a disposable database and scoped role on your configured Postgres server so a coding agent can run the two-session proof without leaving lock experiments in shared development state.

*Published and last updated July 29, 2026.*

The complete proof has five steps:

1. Open two physical connections to the same disposable database.
2. Acquire one exclusive advisory-lock key and require the contender's try-lock to return `false`.
3. Confirm the exact key, database, mode, and holder PID in `pg_locks`.
4. Prove session-lock and transaction-lock release at their different boundaries.
5. Reacquire the key, close every connection, and delete the sandbox.

## In this guide

- [Understand the two advisory-lock lifetimes](#what-postgres-advisory-locks-guarantee)
- [Use the Advisory Lock Proof Contract](#the-advisory-lock-proof-contract)
- [Create a deterministic two-connection harness](#1-create-a-deterministic-advisory-lock-test)
- [Run the proof in a disposable database](#2-run-the-proof-with-pgsandbox)
- [Inspect advisory locks without guessing](#3-read-the-proof-from-pg_locks)
- [Choose session or transaction scope](#4-choose-the-lock-lifetime-deliberately)
- [Prevent lock leaks and flaky tests](#5-keep-advisory-lock-tests-bounded)
- [Record compact review evidence](#pr-ready-advisory-lock-proof)

## What Postgres advisory locks guarantee

PostgreSQL advisory locks protect application-defined resources identified by either one 64-bit integer or two 32-bit integers. PostgreSQL coordinates conflicting lock requests for the same key, but it does not attach business meaning to the key or require other code paths to honor the lock.

The current PostgreSQL [advisory-lock documentation](https://www.postgresql.org/docs/current/explicit-locking.html#ADVISORY-LOCKS) describes two lifetimes:

| Lock family | Lifetime | Manual release | Rollback behavior |
| --- | --- | --- | --- |
| `pg_advisory_lock` / `pg_try_advisory_lock` | Session | `pg_advisory_unlock` or disconnect | Survives rollback |
| `pg_advisory_xact_lock` / `pg_try_advisory_xact_lock` | Transaction | No explicit unlock function | Released on commit or rollback |

Both families can use blocking calls or nonblocking `pg_try_*` calls. The current [advisory-lock function reference](https://www.postgresql.org/docs/current/functions-admin.html#FUNCTIONS-ADVISORY-LOCKS) says the try variants return `true` immediately when the lock is acquired and `false` immediately when a conflicting session already holds it.

Use the nonblocking form for the main integration-test assertion. A blocking call can wait until another mechanism ends it; a finite server or client timeout plus an outer process deadline provides defense in depth. A Boolean denial is faster, easier to classify, and proves contention without making timing the assertion.

The runnable proof below targets **exclusive** advisory locks. PostgreSQL also provides shared variants: shared holders can coexist on one key, while an exclusive request conflicts with them. Test that compatibility matrix separately with three physical sessions if the application uses shared locks.

### Advisory locks are cooperative

An advisory lock does not stop an `UPDATE`, a migration, or a second worker that ignores the key. Every participant in the protected workflow must derive the same key and call a compatible advisory-lock function before entering the critical section.

That makes key design part of the contract. A test should use one deterministic key, identify the resource it represents, and verify that both connections request the same key space. PostgreSQL keeps a single-`bigint` key space separate from the two-`integer` key space, so `pg_advisory_lock(1::bigint)` is not the same resource as `pg_advisory_lock(0, 1)`.

## The Advisory Lock Proof Contract

A reviewable advisory-lock test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Identity | Which application resource does the key represent? | One named constant and one documented derivation rule |
| Contention | Can only one connection hold the exclusive lock? | Holder returns `true`; contender returns `false` without waiting |
| Visibility | Can the test observe the held lock? | A granted `advisory` row in `pg_locks` for the holder PID |
| Lifetime | Does release happen at the intended boundary? | Session lock survives rollback; transaction lock ends on rollback |
| Cleanup | Can another session acquire after release or disconnect? | Final contender acquisition succeeds; sandbox is deleted |

A one-line `pg_try_advisory_lock` example proves only acquisition. This contract also proves contention, ownership visibility, rollback behavior, and final release.

The [PGSandbox MCP tool contract](/docs/mcp-tools/) provides the disposable database lifecycle. The lock harness belongs in one child test process because it must keep two physical database sessions open at the same time.

## 1. Create a deterministic advisory-lock test

The following Psycopg 3 script uses autocommit connections so transaction boundaries are explicit. The current [Psycopg transaction documentation](https://www.psycopg.org/psycopg3/docs/basic/transactions.html#autocommit-transactions) confirms that autocommit avoids an implicit long-lived transaction while still allowing an explicit `BEGIN`. The script first proves that a session lock survives `ROLLBACK` and ends when the holder disconnects. It then proves that a transaction lock ends automatically on rollback.

Save it as `tests/postgres_advisory_lock_proof.py`:

```python
import json
import os

import psycopg


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]
LOCK_KEY = 4_278_190_731


def scalar(conn, query, params=()):
    return conn.execute(query, params).fetchone()[0]


def run_proof():
    contender = psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    )
    holder = psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    )

    try:
        holder_pid = scalar(holder, "SELECT pg_backend_pid()")
        contender_pid = scalar(contender, "SELECT pg_backend_pid()")
        assert holder_pid != contender_pid

        # A session-level lock survives transaction rollback.
        holder.execute("BEGIN")
        assert scalar(
            holder,
            "SELECT pg_try_advisory_lock(%s)",
            (LOCK_KEY,),
        ) is True
        assert scalar(
            contender,
            "SELECT pg_try_advisory_lock(%s)",
            (LOCK_KEY,),
        ) is False

        visible_exact_lock = scalar(
            contender,
            """
            SELECT EXISTS (
              SELECT 1
              FROM pg_locks
              WHERE locktype = 'advisory'
                AND database = (
                  SELECT oid
                  FROM pg_database
                  WHERE datname = current_database()
                )
                AND pid = %s
                AND mode = 'ExclusiveLock'
                AND granted
                AND objsubid = 1
                AND ((classid::bigint << 32) | objid::bigint) = %s
            )
            """,
            (holder_pid, LOCK_KEY),
        )
        assert visible_exact_lock is True

        holder.execute("ROLLBACK")
        assert scalar(
            contender,
            "SELECT pg_try_advisory_lock(%s)",
            (LOCK_KEY,),
        ) is False

        # Session end releases every session-level lock held by that session.
        holder.close()
        assert scalar(
            contender,
            "SELECT pg_try_advisory_lock(%s)",
            (LOCK_KEY,),
        ) is True
        assert scalar(
            contender,
            "SELECT pg_advisory_unlock(%s)",
            (LOCK_KEY,),
        ) is True

        # A transaction-level lock is released by transaction rollback.
        transaction_holder = psycopg.connect(
            DATABASE_URL,
            autocommit=True,
            connect_timeout=5,
        )
        try:
            transaction_holder.execute("BEGIN")
            assert scalar(
                transaction_holder,
                "SELECT pg_try_advisory_xact_lock(%s)",
                (LOCK_KEY,),
            ) is True
            assert scalar(
                contender,
                "SELECT pg_try_advisory_lock(%s)",
                (LOCK_KEY,),
            ) is False

            transaction_holder.execute("ROLLBACK")

            assert scalar(
                contender,
                "SELECT pg_try_advisory_lock(%s)",
                (LOCK_KEY,),
            ) is True
            assert scalar(
                contender,
                "SELECT pg_advisory_unlock(%s)",
                (LOCK_KEY,),
            ) is True
        finally:
            transaction_holder.close()

        return {
            "lockKey": LOCK_KEY,
            "distinctSessions": holder_pid != contender_pid,
            "visibleExactLock": visible_exact_lock,
            "sessionLockSurvivedRollback": True,
            "sessionLockReleasedOnDisconnect": True,
            "transactionLockReleasedOnRollback": True,
            "finalAcquisitionSucceeded": True,
        }
    finally:
        if not holder.closed:
            holder.execute("SELECT pg_advisory_unlock_all()")
            holder.close()
        contender.execute("SELECT pg_advisory_unlock_all()")
        contender.close()


print(json.dumps(run_proof(), sort_keys=True))
```

The fixed key is safe for a task-scoped database, but an application should define a stable collision-resistant derivation rule for its real resource names.

Prerequisites are a configured PGSandbox Postgres profile, Python, `uv`, and capacity for at least two scoped connections. The command installs Psycopg into an ephemeral `uv` environment. A passing child prints the lock key plus every Boolean proof field set to `true`.

## 2. Run the proof with PGSandbox

Run the complete two-session harness as one bounded child process:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint "advisory lock proof" \
  --ttl-minutes 30 \
  --cleanup always \
  --timeout-seconds 60 \
  --result-format json \
  -- uv run --with 'psycopg[binary]' \
    python tests/postgres_advisory_lock_proof.py
```

`with-database` creates a tracked database and scoped login role, injects `PGSANDBOX_DATABASE_URL`, captures bounded credential-redacted output, and applies the requested cleanup policy. The [agent test-session guide](/blog/run-integration-tests-disposable-postgres-database/) explains the versioned result and cleanup fields.

Use `--cleanup always` for CI and unattended agent runs. During active debugging, `--cleanup on-success` can retain a failed sandbox until its TTL, but delete it explicitly after inspection.

## 3. Read the proof from `pg_locks`

The PostgreSQL [`pg_locks` view](https://www.postgresql.org/docs/current/view-pg-locks.html) exposes one row per active lockable object, requested mode, and relevant process. Advisory rows use `locktype = 'advisory'`; `pid` identifies the holding or waiting backend; and `granted` distinguishes ownership from a wait request.

The harness matches the holder PID, current database, exclusive mode, granted state, key form, and reconstructed `bigint` value. Those checks prevent an unrelated advisory lock held by the same session from satisfying the visibility assertion.

For operational inspection, include the database and both key forms:

```sql
SELECT
  database,
  pid,
  mode,
  granted,
  objsubid,
  classid,
  objid
FROM pg_locks
WHERE locktype = 'advisory'
ORDER BY pid, classid, objid;
```

PostgreSQL documents that a single `bigint` key is split across `classid` and `objid` with `objsubid = 1`; a two-integer key uses `objsubid = 2`. Advisory locks are local to each database, so the same numeric key in another database is a different lock target.

Do not turn the catalog query into the only assertion. A granted row proves ownership at one instant. The contender's `false` result proves conflict behavior, and the final successful acquisition proves release.

## 4. Choose the lock lifetime deliberately

Prefer transaction-level advisory locks when the critical section is exactly one database transaction. They release on both commit and rollback, so application error paths cannot forget a matching unlock.

Use session-level locks only when the protected work must span transaction boundaries. They are appropriate for some leader-election, scheduler, and cross-transaction coordination patterns, but the connection becomes the owner. In a connection pool, returning that connection does not necessarily end its PostgreSQL session.

That pooling boundary is easy to miss. A session-level lock acquired through a pooled connection can outlive the request that borrowed it. The application must unlock in a `finally` path before returning the connection, or intentionally close the physical session. The [connection-pool failure testing guide](/blog/test-postgres-connection-pool-failures/) covers how client-pool state differs from PostgreSQL backend state.

### Do not mix key derivation rules

Write one function that maps a resource identity to the advisory key and use it everywhere. Include a namespace or resource type so an invoice lock cannot collide with a tenant-migration lock.

Avoid relying on a language runtime's default string hash unless its stability and width are documented as part of the contract. If you hash names, choose an explicit algorithm, specify how bytes become signed PostgreSQL integers, and test known input-output vectors.

## 5. Keep advisory-lock tests bounded

### Use try-locks for the main assertion

`pg_try_advisory_lock` and `pg_try_advisory_xact_lock` return immediately. That removes scheduler timing from the contention assertion.

If production uses blocking acquisition, keep one separate test with a short transaction-local `lock_timeout` and an outer process timeout. The [deadlock and lock-timeout guide](/blog/test-postgres-deadlocks-lock-timeouts/) shows how to classify `55P03` without waiting indefinitely.

### Close connections in `finally`

Session-level locks stack: repeated successful acquisitions require the same number of unlocks. `pg_advisory_unlock_all()` is useful as defensive test cleanup for the current session, and PostgreSQL also releases session locks when that session ends.

Explicit cleanup still matters. It makes the intended boundary visible and prevents a reused test connection from carrying state into another case.

### Do not acquire locks from an unordered limited query

PostgreSQL warns that expression evaluation order can cause `pg_advisory_lock(id)` to run before a query's `LIMIT` is applied. Select the bounded key set in a subquery first, then acquire locks from that result.

### Do not treat advisory locks as data constraints

Advisory locks work only when every writer cooperates. Use `UNIQUE`, `FOREIGN KEY`, `EXCLUDE`, or row-locking semantics when PostgreSQL itself must enforce the invariant. An advisory lock may coordinate a workflow around those constraints; it should not silently replace them.

## PR-ready advisory-lock proof

When a patch changes concurrent code, record a compact result like this:

```text
Postgres advisory-lock proof
- Target: PostgreSQL=<major>, sandbox=<safe database ID>
- Identity: key=<non-secret integer>, resource=<documented meaning>
- Topology: two independent physical connections
- Contention: holder=true, contender=false, nonblocking try-lock
- Visibility: granted advisory row found for holder PID
- Session lifetime: survived rollback, released on disconnect
- Transaction lifetime: released on rollback
- Final state: contender acquired and released the same key
- Session: status=<status>, child exit=<code>, elapsed=<duration>
- Cleanup: policy=always, deleted=<yes/no>, error=<stable code or none>
```

To run this against your own configured Postgres server, [install and set up PGSandbox](/docs/install/), add the harness to the repository under test, and keep the structured proof with the concurrency-sensitive PR.

## Frequently asked questions

### How do you test a PostgreSQL advisory lock?

Open two physical connections to the same database. Acquire one fixed key with a nonblocking advisory-lock function on the first connection, assert the second returns `false`, inspect a granted advisory row in `pg_locks`, release at the intended session or transaction boundary, then assert the second connection can acquire the key.

### What is the difference between session and transaction advisory locks?

A session-level advisory lock remains held until it is explicitly unlocked or the PostgreSQL session ends, even if the transaction that acquired it rolls back. A transaction-level advisory lock is released automatically when its transaction commits or rolls back and has no manual unlock function.

### Are Postgres advisory locks released when a connection closes?

Yes. PostgreSQL releases session-level advisory locks when the owning session ends. After an abrupt network loss, release occurs when PostgreSQL detects that the session has ended. A pool may keep the physical session alive after application code returns a connection, so pooled code should not treat a logical checkout boundary as a disconnect.

### Can I see advisory locks in `pg_locks`?

Yes. Filter `pg_locks` with `locktype = 'advisory'`. Use `pid`, `mode`, and `granted` to identify holders and waiters. PostgreSQL stores a `bigint` key across `classid` and `objid` with `objsubid = 1`; two-integer keys use `objsubid = 2`.

### Should advisory-lock tests use blocking calls?

Use nonblocking `pg_try_advisory_*` calls for the main contention assertion. They return a deterministic Boolean instead of waiting. If blocking behavior is part of the application contract, test it separately with a short `lock_timeout` and a bounded outer process deadline.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Test Postgres advisory locks safely",
      "description": "Prove exclusive advisory-lock contention, visibility, lifetime, and cleanup with independent Postgres sessions.",
      "url": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-advisory-locks/",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Open two physical connections", "text": "Connect two independent sessions to the same disposable Postgres database."},
        {"@type": "HowToStep", "position": 2, "name": "Prove contention", "text": "Acquire one fixed key on the holder and require the contender's nonblocking try-lock to return false."},
        {"@type": "HowToStep", "position": 3, "name": "Inspect the held lock", "text": "Confirm a granted advisory row in pg_locks for the holder backend PID."},
        {"@type": "HowToStep", "position": 4, "name": "Verify both lifetimes", "text": "Prove a session lock survives rollback and ends on disconnect, while a transaction lock ends on rollback."},
        {"@type": "HowToStep", "position": 5, "name": "Verify cleanup", "text": "Require a final acquisition by the contender, close both connections, and delete the disposable database."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do you test a PostgreSQL advisory lock?", "acceptedAnswer": {"@type": "Answer", "text": "Use two physical connections to the same database. Acquire one fixed key on the first, require the second try-lock to return false, inspect pg_locks, release at the intended boundary, and require the second connection to acquire it."}},
        {"@type": "Question", "name": "What is the difference between session and transaction advisory locks?", "acceptedAnswer": {"@type": "Answer", "text": "Session advisory locks survive rollback and end only after explicit unlock or session end. Transaction advisory locks are released automatically on commit or rollback."}},
        {"@type": "Question", "name": "Are Postgres advisory locks released when a connection closes?", "acceptedAnswer": {"@type": "Answer", "text": "Yes. PostgreSQL releases session-level advisory locks when the owning session ends. After an abrupt network loss, release occurs when PostgreSQL detects that the session has ended."}},
        {"@type": "Question", "name": "Can I see advisory locks in pg_locks?", "acceptedAnswer": {"@type": "Answer", "text": "Yes. Filter pg_locks for locktype advisory and inspect pid, mode, and granted to identify active holders and waiters."}},
        {"@type": "Question", "name": "Should advisory-lock tests use blocking calls?", "acceptedAnswer": {"@type": "Answer", "text": "Use nonblocking pg_try_advisory functions for the main assertion. Test blocking behavior separately with a short lock timeout and an outer process deadline."}}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test Postgres Advisory Locks Safely", "item": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-advisory-locks/"}
      ]
    }
  ]
}
</script>
