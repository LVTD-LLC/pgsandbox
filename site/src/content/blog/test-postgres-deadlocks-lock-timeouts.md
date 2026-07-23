---
title: "How to Test Postgres Deadlocks and Lock Timeouts Safely"
excerpt: "Reproduce PostgreSQL deadlocks and lock timeouts with two coordinated connections, assert the right SQLSTATE, and clean up the disposable database."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-23"
updatedAt: "2026-07-23T06:00:00Z"
tags: ["Postgres", "deadlocks", "lock timeout", "integration testing", "coding agents"]
category: "Engineering"
metaTitle: "Postgres Deadlock Testing in a Disposable Database"
metaDescription: "Test Postgres deadlocks and lock timeouts with deterministic connection coordination, SQLSTATE assertions, rollback checks, and disposable cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-deadlocks-lock-timeouts/"
heroImageUrl: ""
featured: false
sortOrder: 145
---
Postgres deadlock testing needs two independent connections, explicit coordination, and assertions about the error and the resulting database state. Force the connections to acquire the same locks in opposite order, expect exactly one `40P01` deadlock victim, verify the other transaction commits, and run the whole test in a disposable database with a bounded outer timeout.

Test lock timeouts separately. A one-way wait that exceeds `lock_timeout` should produce `55P03`, not `40P01`. Treating those outcomes as the same error hides whether the application handled a real cycle or merely stopped waiting.

This guide packages the evidence into a **Concurrency Proof Contract** with five fields: topology, coordination, classification, recovery, and cleanup. The contract makes a concurrency test useful to both a coding agent and the reviewer deciding whether to merge its change.

## In this guide

- [Distinguish a deadlock from a lock timeout](#deadlock-vs-lock-timeout)
- [Use the Concurrency Proof Contract](#the-concurrency-proof-contract)
- [Run the test inside one disposable session](#why-the-test-needs-one-child-process)
- [Create a deterministic two-connection harness](#1-create-the-two-connection-test-harness)
- [Run the harness with PGSandbox](#2-run-the-harness-in-a-disposable-database)
- [Read the result without guessing](#3-assert-the-error-and-the-database-state)
- [Inspect a blocked test safely](#4-inspect-blockers-without-changing-the-test)
- [Test transaction retry policy](#5-test-retry-policy-above-the-database-harness)
- [Record PR-ready evidence](#pr-ready-deadlock-test-proof)

## Deadlock vs lock timeout

A PostgreSQL deadlock is a cycle. Transaction A holds a lock needed by transaction B while transaction B holds a lock needed by transaction A. PostgreSQL's [explicit-locking documentation](https://www.postgresql.org/docs/current/explicit-locking.html#LOCKING-DEADLOCKS) says the server detects the cycle and aborts one transaction so the other can finish. The victim is not predictable.

A lock timeout is a budget. One transaction can hold a row lock while another waits for it without forming a cycle. PostgreSQL's [`lock_timeout` reference](https://www.postgresql.org/docs/current/runtime-config-client.html#GUC-LOCK-TIMEOUT) says the waiting statement is aborted after the configured duration for a lock acquisition attempt. The default is zero, which disables that timeout.

The two paths need different tests:

| Condition | Required shape | Expected SQLSTATE | Useful assertion |
| --- | --- | --- | --- |
| Deadlock | Two or more transactions form a wait cycle | `40P01` (`deadlock_detected`) | One victim rolls back; another transaction can commit |
| Lock timeout | A statement waits longer than its lock budget | `55P03` (`lock_not_available`) | The waiting statement fails within the configured budget |
| Serialization failure | Concurrent result cannot be serialized at the selected isolation level | `40001` (`serialization_failure`) | Retry the complete transaction from a fresh snapshot |
| Statement cancellation | The statement exceeds `statement_timeout` or is canceled | `57014` (`query_canceled`) | Classify it separately from lock acquisition failure |

Those codes come from PostgreSQL's current [SQLSTATE appendix](https://www.postgresql.org/docs/current/errcodes-appendix.html). Branch on the code, not localized error text. The [PGSandbox error-handling guide](/blog/postgres-mcp-server-error-handling-coding-agents/) applies the same rule to MCP tool responses.

`deadlock_timeout` is different again. It controls how long PostgreSQL waits on a lock before running the deadlock detector. The [lock-management settings reference](https://www.postgresql.org/docs/current/runtime-config-locks.html) documents a default of one second and notes that the check has a cost. An application test normally should not rewrite this cluster-level diagnostic setting. Bound the test process and accept the server's configured detector interval.

## The Concurrency Proof Contract

A passing deadlock test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Topology | Which independent sessions and locks formed the cycle? | Two connection names and opposite row order |
| Coordination | How did the test make the ordering deterministic? | A barrier reached after each first lock |
| Classification | Which database condition occurred? | SQLSTATE `40P01` or `55P03`, never a text match |
| Recovery | What happened after the error? | Victim rollback, surviving commit, and final data invariant |
| Cleanup | What state and connections remain? | Closed clients plus deleted or deliberately retained sandbox |

This framework is the information gain over a copy-paste pair of SQL sessions. The SQL example shows that a deadlock can happen. The contract proves that application code recognized the right failure, left the connection usable, preserved a valid database state, and did not turn the fixture into shared infrastructure.

## Why the test needs one child process

The concurrency harness must keep two connections and two transactions open at the same time. That means it belongs inside the repository's test process.

PGSandbox's [`run_sql` implementation](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/2e709adbd78189ee561d5c394151d97e377f1f6b/rust-src/postgres.rs#L2640-L2672) opens a sandbox connection for one tool call and drops it before returning. Separate `run_sql` calls are therefore useful for bounded setup and proof queries, but they are not a multi-session transaction coordinator. Trying to alternate `BEGIN` and `UPDATE` across independent calls will not preserve the intended open transactions.

Use `pgsandbox with-database` instead. The command creates one disposable database and scoped role, injects `DATABASE_URL`, `PGSANDBOX_DATABASE_URL`, and standard libpq variables into one child process, then supervises its exit and cleanup. The [one-shot integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) covers the full session result contract.

This is a product-led boundary, not a workaround:

- PGSandbox owns database and credential lifecycle.
- The repository test runner owns connection concurrency.
- PostgreSQL owns lock detection and transaction semantics.
- The PR proof records the safe result without exposing a database URL.

## 1. Create the two-connection test harness

The following Python harness uses Psycopg 3 because the coordination is visible in one file. The same shape works with Node `pg`, JDBC, Go `database/sql`, Rust SQLx, or another driver that exposes SQLSTATE.

Save it as `tests/postgres_lock_proof.py`:

```python
import json
import os
import queue
import threading

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]


def reset_fixture():
    with psycopg.connect(DATABASE_URL, autocommit=True) as conn:
        conn.execute("DROP TABLE IF EXISTS lock_proof_accounts")
        conn.execute(
            """
            CREATE TABLE lock_proof_accounts (
                id integer PRIMARY KEY,
                touches integer NOT NULL DEFAULT 0
            )
            """
        )
        conn.execute(
            "INSERT INTO lock_proof_accounts (id) VALUES (1), (2)"
        )


def deadlock_worker(name, first_id, second_id, barrier, results):
    try:
        with psycopg.connect(DATABASE_URL, autocommit=True) as conn:
            with conn.transaction():
                conn.execute(
                    "SELECT set_config('application_name', %s, true)",
                    (f"deadlock-proof-{name}",),
                )
                conn.execute(
                    "UPDATE lock_proof_accounts "
                    "SET touches = touches + 1 WHERE id = %s",
                    (first_id,),
                )
                barrier.wait(timeout=5)
                conn.execute(
                    "UPDATE lock_proof_accounts "
                    "SET touches = touches + 1 WHERE id = %s",
                    (second_id,),
                )
        results.put({"worker": name, "status": "committed", "sqlstate": None})
    except errors.DeadlockDetected as exc:
        results.put(
            {"worker": name, "status": "deadlock", "sqlstate": exc.sqlstate}
        )
    except Exception as exc:
        results.put(
            {"worker": name, "status": "unexpected", "error": type(exc).__name__}
        )


def prove_deadlock():
    barrier = threading.Barrier(2)
    results = queue.Queue()
    workers = [
        threading.Thread(
            target=deadlock_worker,
            args=("a", 1, 2, barrier, results),
            daemon=True,
        ),
        threading.Thread(
            target=deadlock_worker,
            args=("b", 2, 1, barrier, results),
            daemon=True,
        ),
    ]

    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join(timeout=10)

    if any(worker.is_alive() for worker in workers):
        raise AssertionError("deadlock detector did not finish within 10 seconds")

    outcomes = sorted((results.get() for _ in workers), key=lambda row: row["worker"])
    statuses = sorted(row["status"] for row in outcomes)
    assert statuses == ["committed", "deadlock"], outcomes
    assert [row["sqlstate"] for row in outcomes if row["status"] == "deadlock"] == [
        "40P01"
    ]

    with psycopg.connect(DATABASE_URL) as conn:
        total_touches = conn.execute(
            "SELECT sum(touches) FROM lock_proof_accounts"
        ).fetchone()[0]
    assert total_touches == 2
    return outcomes


def hold_row_lock(locked, release):
    with psycopg.connect(DATABASE_URL, autocommit=True) as conn:
        with conn.transaction():
            conn.execute(
                "SELECT id FROM lock_proof_accounts WHERE id = 1 FOR UPDATE"
            )
            locked.set()
            if not release.wait(timeout=5):
                raise AssertionError("lock holder was not released")


def prove_lock_timeout():
    locked = threading.Event()
    release = threading.Event()
    holder = threading.Thread(
        target=hold_row_lock,
        args=(locked, release),
        daemon=True,
    )
    holder.start()
    assert locked.wait(timeout=5), "holder did not acquire the row lock"

    outcome = None
    try:
        with psycopg.connect(DATABASE_URL, autocommit=True) as conn:
            with conn.transaction():
                conn.execute("SET LOCAL lock_timeout = '250ms'")
                conn.execute(
                    "UPDATE lock_proof_accounts "
                    "SET touches = touches + 1 WHERE id = 1"
                )
    except errors.LockNotAvailable as exc:
        outcome = {"status": "lock_timeout", "sqlstate": exc.sqlstate}
    finally:
        release.set()
        holder.join(timeout=5)

    assert not holder.is_alive(), "holder connection did not close"
    assert outcome == {"status": "lock_timeout", "sqlstate": "55P03"}
    return outcome


if __name__ == "__main__":
    reset_fixture()
    proof = {
        "deadlock": prove_deadlock(),
        "lockTimeout": prove_lock_timeout(),
    }
    print(json.dumps(proof, sort_keys=True))
```

The important line is the barrier after the first update. Worker A holds row 1. Worker B holds row 2. Only after both locks exist does each worker request the other's row. Sleep-only tests cannot prove that ordering and tend to become timing-dependent on CI.

The test does not assert which worker PostgreSQL aborts. The official deadlock documentation says that choice is difficult to predict. It asserts the stable invariant: one transaction receives `40P01`, one commits, and only the committed transaction's two increments remain.

The timeout test has a different topology. One connection holds row 1 with `SELECT ... FOR UPDATE`; the other uses transaction-scoped `lock_timeout` and attempts an update. PostgreSQL's [`SELECT` locking-clause documentation](https://www.postgresql.org/docs/current/sql-select.html#SQL-FOR-UPDATE-SHARE) says the row lock lasts until the transaction ends. [`SET LOCAL`](https://www.postgresql.org/docs/current/sql-set.html) keeps the 250 ms budget scoped to the waiting transaction, so it does not leak into later work on the connection.

## 2. Run the harness in a disposable database

Run the proof against a real PostgreSQL major with an outer process timeout and unconditional cleanup:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 15 \
  --cleanup always \
  --timeout-seconds 45 \
  --result-format json \
  -- uv run --with 'psycopg[binary]' python tests/postgres_lock_proof.py
```

PGSandbox supplies the connection to the child. Do not print `PGSANDBOX_DATABASE_URL`, `DATABASE_URL`, `PGPASSWORD`, or a derived DSN. The structured session output carries safe database identity, child status, bounded redacted output, expiry, and cleanup state.

Use `--cleanup always` for CI and unattended agent runs. During active debugging, `--cleanup on-success` can retain a failed sandbox until manual deletion or TTL expiry. The [sandbox TTL guide](/blog/postgres-sandbox-ttl-values/) explains how to choose enough review and recovery time without creating a long-lived test database.

The outer 45-second timeout protects the runner if the coordination code breaks or the server has an unusual detector setting. It is not the assertion for either database error. The database-side SQLSTATE remains the classification evidence.

## 3. Assert the error and the database state

A useful result contains more than "deadlock detected."

For the deadlock path, assert:

1. Both workers reached the barrier after acquiring their first row lock.
2. Exactly one worker returned `40P01`.
3. Exactly one worker committed.
4. The victim's first update rolled back.
5. The final invariant reflects one complete transaction, not half of each.
6. Both connection contexts closed.

For the lock-timeout path, assert:

1. The holder acquired the row lock before the waiter started.
2. The waiter's transaction used `SET LOCAL lock_timeout`.
3. The waiter returned `55P03`.
4. The holder was deliberately released.
5. Both connection contexts closed.

After a database error, do not continue issuing statements in the failed transaction as if nothing happened. PostgreSQL documents that an errored transaction remains aborted until it is rolled back or recovered to a savepoint. Driver transaction contexts make that boundary explicit: the exception leaves the block, the driver rolls back, and any retry begins as a new transaction.

That is why the test checks final data. Matching `40P01` proves classification. A correct final invariant proves recovery.

## 4. Inspect blockers without changing the test

When the test hangs before PostgreSQL reports the expected outcome, inspect wait state from a separate connection. Do not add random sleeps until it appears to pass.

`pg_stat_activity` shows active queries and wait categories. `pg_blocking_pids()` identifies sessions ahead of a blocked backend in the lock wait graph:

```sql
SELECT
    pid,
    application_name,
    state,
    wait_event_type,
    wait_event,
    pg_blocking_pids(pid) AS blocked_by,
    left(query, 160) AS query
FROM pg_stat_activity
WHERE datname = current_database()
  AND application_name LIKE 'deadlock-proof-%'
ORDER BY application_name;
```

The [system-information function reference](https://www.postgresql.org/docs/current/functions-info.html) notes that `pg_blocking_pids()` can return duplicate client-visible PIDs for parallel queries and that frequent calls briefly take exclusive access to lock-manager shared state. Use it as a diagnostic sample, not a tight polling loop.

`pg_locks` can add lock-mode detail, but the [view documentation](https://www.postgresql.org/docs/current/view-pg-locks.html) warns that row locks normally do not appear directly. A backend waiting for a row often appears as waiting on the holder's transaction ID. Do not fail a test because a guessed row-lock record is absent.

For a disposable test, the diagnostic questions are narrow:

- Did both named connections reach PostgreSQL?
- Did each acquire its first lock?
- Is each active backend waiting on a `Lock` event?
- Does the blocker graph match the intended topology?
- Did an outer timeout kill the child before the detector could report?

## 5. Test retry policy above the database harness

PostgreSQL's deadlock guidance recommends consistent lock ordering as the primary prevention. If a cycle cannot be ruled out, retry the complete transaction, not the single failed statement.

Keep two tests:

- The integration harness proves that the driver exposes a real PostgreSQL `40P01` and rolls back the victim.
- Unit tests prove retry count, backoff, jitter, idempotency, and the final error when the retry budget is exhausted.

Do not make every retry-policy case depend on a live deadlock. A real cycle is valuable once because it proves driver and database behavior. It is a poor clock for testing five backoff branches.

The retry wrapper should accept a transaction function. On `40P01`, it should discard the failed transaction, wait according to a bounded policy, open a new transaction, and replay the whole business operation. It should not retry syntax errors, constraint violations, authentication failures, or every `55P03` without considering why the blocker exists.

If the operation can emit an external side effect, add an idempotency key or transactional outbox before enabling retries. A database transaction can roll back database writes. It cannot retract an email or API call already sent outside that transaction.

## Common Postgres deadlock testing mistakes

### Using one connection

One connection cannot hold two independent transactions at once. Use two checked-out clients or two direct connections.

### Coordinating with sleep

Sleep guesses at scheduling. A barrier or pair of events proves both workers reached the intended lock boundary before either continues.

### Expecting a specific victim

PostgreSQL does not promise which transaction it will abort. Assert one victim and one survivor.

### Treating every timeout as a deadlock

`40P01`, `55P03`, `40001`, and `57014` represent different recovery branches. Preserve SQLSTATE.

### Retrying inside the failed transaction

The transaction has already failed. Roll it back and replay the complete transaction in a fresh context.

### Running the proof against a shared database

A concurrency test deliberately blocks and aborts work. Run it against a task-scoped [database sandbox](/blog/what-is-database-sandbox/) with its own role, TTL, and cleanup path.

### Alternating MCP SQL calls to imitate sessions

PGSandbox `run_sql` calls are bounded request/response operations. Use the repository test process for concurrent connections and use the [bounded SQL workflow](/blog/postgres-run-sql-bounded-results/) for setup or compact post-test evidence.

## PR-ready deadlock test proof

Record a compact proof with a concurrency-related patch:

```text
Postgres concurrency proof
- Target: PostgreSQL=<major>, sandbox=<safe database ID>
- Topology: worker-a locks 1→2; worker-b locks 2→1
- Coordination: barrier after first UPDATE
- Deadlock: one 40P01 victim, one committed transaction
- Lock timeout: one 55P03 after transaction-local 250ms budget
- Recovery: victim rolled back; final touches=2
- Command: uv run --with psycopg[binary] python tests/postgres_lock_proof.py
- Session: status=<status>, child exit=<code>, elapsed=<duration>
- Cleanup: policy=always, deleted=<yes/no>, error=<stable code or none>
```

This gives a reviewer the concurrency shape, database classification, post-error state, and lifecycle result. It omits credentials and unbounded logs.

## Frequently asked questions

### How do you reproduce a PostgreSQL deadlock reliably?

Use two independent transactions. Have each transaction lock a different row, synchronize them with a barrier, then make each request the row held by the other. Assert that exactly one transaction receives SQLSTATE `40P01` and the other commits. Do not coordinate the workers with sleep alone.

### What is the difference between `deadlock_timeout` and `lock_timeout`?

`deadlock_timeout` controls when PostgreSQL checks a lock wait for a deadlock cycle. `lock_timeout` limits how long one statement waits to acquire a lock. A deadlock can produce `40P01`; a lock acquisition timeout produces `55P03`.

### Should a test lower `deadlock_timeout`?

Usually no. It is a server diagnostic setting with a default of one second, and changing it requires elevated permission or explicit `SET` privilege. Keep the application test unprivileged, use the server's configured value, and apply a bounded outer process timeout.

### Can separate PGSandbox `run_sql` calls reproduce a deadlock?

No reliable multi-session transaction can be built from separate calls because each call opens and closes its own connection. Run a child test process through `pgsandbox with-database` and let that process own both concurrent connections.

### Should an application retry a deadlock?

It may retry SQLSTATE `40P01`, but it must roll back and replay the complete transaction with a bounded retry policy. Consistent lock ordering and short transactions remain the primary fixes. Retry logic also needs idempotency for any external side effects.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Test Postgres deadlocks and lock timeouts safely",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Create two independent connections", "text": "Run two database transactions inside one repository test process."},
        {"@type": "HowToStep", "position": 2, "name": "Coordinate opposite lock order", "text": "Lock one row per worker, wait at a barrier, then request the other worker's row."},
        {"@type": "HowToStep", "position": 3, "name": "Assert SQLSTATE", "text": "Expect one 40P01 deadlock victim and test 55P03 lock timeout in a separate one-way wait."},
        {"@type": "HowToStep", "position": 4, "name": "Verify recovery", "text": "Confirm victim rollback, survivor commit, valid final data, and closed connections."},
        {"@type": "HowToStep", "position": 5, "name": "Run in a disposable database", "text": "Use pgsandbox with-database with a bounded timeout, explicit cleanup policy, and structured result."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do you reproduce a PostgreSQL deadlock reliably?", "acceptedAnswer": {"@type": "Answer", "text": "Use two independent transactions, lock different rows, synchronize with a barrier, then request the rows in opposite order. Assert one SQLSTATE 40P01 victim and one commit."}},
        {"@type": "Question", "name": "What is the difference between deadlock_timeout and lock_timeout?", "acceptedAnswer": {"@type": "Answer", "text": "deadlock_timeout controls when PostgreSQL checks for a deadlock cycle. lock_timeout limits how long one statement waits to acquire a lock."}},
        {"@type": "Question", "name": "Should a test lower deadlock_timeout?", "acceptedAnswer": {"@type": "Answer", "text": "Usually no. Keep the test unprivileged, use the server's configured detector interval, and bound the outer test process."}},
        {"@type": "Question", "name": "Can separate PGSandbox run_sql calls reproduce a deadlock?", "acceptedAnswer": {"@type": "Answer", "text": "No reliable multi-session transaction can be built from separate calls. Run one child test process through pgsandbox with-database and let it own both connections."}},
        {"@type": "Question", "name": "Should an application retry a deadlock?", "acceptedAnswer": {"@type": "Answer", "text": "It may retry SQLSTATE 40P01 after rolling back and replaying the complete transaction with a bounded policy and idempotent side effects."}}
      ]
    }
  ]
}
</script>
