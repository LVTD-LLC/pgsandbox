---
title: "How to Test PostgreSQL Transaction Isolation Levels"
excerpt: "Prove Read Committed snapshot drift, Repeatable Read stability, and Serializable write-skew protection with controlled concurrent transactions."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-30"
updatedAt: "2026-07-30T06:00:00Z"
tags: ["Postgres", "transaction isolation", "concurrency testing", "integration testing", "coding agents"]
category: "Engineering"
metaTitle: "Test PostgreSQL Transaction Isolation Levels"
metaDescription: "Test PostgreSQL isolation levels with controlled snapshots, a write-skew schedule, SQLSTATE 40001 assertions, and disposable cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-transaction-isolation-levels/"
heroImageUrl: ""
featured: false
sortOrder: 151
---
Test PostgreSQL transaction isolation levels with controlled two-connection schedules, not sleeps or a single happy-path transaction. A useful proof should make Read Committed observe a concurrent commit, make Repeatable Read preserve its earlier snapshot, and make Serializable reject a write-skew result that Repeatable Read allows.

Those outcomes answer different questions. Snapshot stability tells you whether repeated reads can change. Serializable safety tells you whether the final committed state could have come from some one-at-a-time execution. Treating both as "stronger isolation" hides the application invariant you actually need.

This guide packages the checks into an **Isolation Proof Matrix**: level, schedule, observation, classification, and cleanup. PGSandbox MCP creates a disposable database and scoped role on your configured PostgreSQL server so a coding agent can run the proof without sharing state with another task.

*Published and last updated July 30, 2026.*

The complete proof has five steps:

1. Start every test from a known fixture in a disposable database.
2. Coordinate two physical connections at explicit transaction boundaries.
3. Run a snapshot probe under Read Committed and Repeatable Read.
4. Run a write-skew probe under Repeatable Read and Serializable.
5. Assert SQLSTATE and final state, then delete the sandbox.

## In this guide

- [Understand PostgreSQL's three implemented isolation behaviors](#what-postgresql-transaction-isolation-levels-guarantee)
- [Use the Isolation Proof Matrix](#the-isolation-proof-matrix)
- [Create the deterministic harness](#1-create-a-deterministic-isolation-test)
- [Run it with PGSandbox](#2-run-the-proof-in-a-disposable-postgres-database)
- [Interpret the outcomes](#3-interpret-the-isolation-proof)
- [Choose the level from the invariant](#4-choose-isolation-from-the-application-invariant)
- [Keep the test deterministic](#5-keep-isolation-tests-deterministic)
- [Record PR-ready evidence](#pr-ready-isolation-proof)

## What PostgreSQL transaction isolation levels guarantee

PostgreSQL accepts the four SQL names `READ UNCOMMITTED`, `READ COMMITTED`, `REPEATABLE READ`, and `SERIALIZABLE`, but its current [transaction-isolation documentation](https://www.postgresql.org/docs/current/transaction-iso.html) defines only three distinct behaviors. `READ UNCOMMITTED` maps to `READ COMMITTED` because PostgreSQL's multiversion concurrency control does not expose dirty reads.

| Requested level | Snapshot boundary in PostgreSQL | Key allowed behavior |
| --- | --- | --- |
| Read Uncommitted | Same as Read Committed | No dirty reads; nonrepeatable reads remain possible |
| Read Committed | New snapshot for each statement | Two `SELECT` statements in one transaction may see different committed values |
| Repeatable Read | Snapshot fixed by the first query or data-changing statement | Stable reads and no phantom reads in PostgreSQL, but serialization anomalies remain possible |
| Serializable | Repeatable Read snapshot plus dependency checks | Unsafe concurrent results are rejected so committed transactions match a serial order |

Read Committed is PostgreSQL's default. Each plain `SELECT` sees rows committed before that statement began, so a later statement in the same transaction can observe a concurrent commit. This is often the right behavior for short operations on predetermined rows, but it is not a stable transaction-wide view.

Repeatable Read fixes the snapshot at the first query or data-modification statement. Successive reads see the same committed state even after another transaction commits. PostgreSQL's implementation is stronger than the SQL minimum because it also prevents phantom reads, but it can still allow a serialization anomaly such as write skew.

Serializable uses the same snapshot model and monitors read/write dependencies. If accepting every concurrent transaction would create a result inconsistent with all possible serial orders, PostgreSQL aborts one with SQLSTATE `40001`. The application must replay the complete transaction from a fresh snapshot.

The current [`SET TRANSACTION` reference](https://www.postgresql.org/docs/current/sql-set-transaction.html) also sets a timing constraint for the test: choose the isolation level before the first query or data-modification statement. A harness that runs `SELECT` and then changes isolation is testing an error path, not the requested level.

### Stable snapshots and serializable outcomes are not the same

A stable view answers, "Will this transaction's repeated reads change?" A serializable result answers, "Could the committed outcome have happened if these transactions ran one at a time?"

The distinction is easiest to see with write skew. Imagine two on-call engineers. Each transaction reads that two engineers are available, then marks a different engineer off call. Under Repeatable Read, both decisions can use stable snapshots and both can commit, leaving nobody on call. Under Serializable, PostgreSQL detects the incompatible read/write dependencies and rejects one transaction.

That is why the existing [serialization retry guide](/blog/test-postgres-serialization-failure-retries/) focuses on replay after `40001`, while this guide focuses on proving which level permits or rejects a specific schedule.

## The Isolation Proof Matrix

A reviewable isolation test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Level | Which isolation level actually ran? | `SHOW transaction_isolation` from inside each transaction |
| Schedule | Did the intended concurrent order occur? | Separate physical connections and barriers or explicit commit boundaries |
| Observation | What did each transaction read and write? | Fixed scalar values and a final-state query |
| Classification | Was an abort the expected database condition? | Driver SQLSTATE `40001`, never an English message match |
| Cleanup | Were transactions, connections, and the task database removed? | Closed connection contexts plus structured PGSandbox cleanup |

The matrix prevents a common false positive: setting an isolation constant and asserting that the function returned. The proof must expose a behavior that would change under another level.

PGSandbox owns the database lifecycle, not the concurrent schedule. The child test process owns both connections, barriers, transaction boundaries, and the business invariant. The [PGSandbox MCP tool contract](/docs/mcp-tools/) keeps lifecycle authority separate from task-role SQL.

## 1. Create a deterministic isolation test

The following Psycopg 3 harness runs two probes. The first performs two reads around a committed update. The second coordinates two transactions that each make a decision from the same two-row on-call set.

Connections use `autocommit=True` so every transaction boundary is explicit. Psycopg's current [transaction documentation](https://www.psycopg.org/psycopg3/docs/basic/transactions.html#transaction-characteristics) notes that transaction characteristics apply to explicit transaction blocks on autocommit connections and must be selected before a transaction is active.

Save this as `tests/postgres_isolation_proof.py`:

```python
import json
import os
import threading
from concurrent.futures import ThreadPoolExecutor

import psycopg


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]
LEVELS = {"READ COMMITTED", "REPEATABLE READ", "SERIALIZABLE"}


def connect():
    return psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    )


def begin(conn, level):
    if level not in LEVELS:
        raise ValueError(f"unsupported isolation level: {level}")
    conn.execute(f"BEGIN ISOLATION LEVEL {level}")
    conn.execute("SET LOCAL statement_timeout = '5s'")
    conn.execute("SET LOCAL lock_timeout = '2s'")
    actual = conn.execute("SHOW transaction_isolation").fetchone()[0]
    assert actual == level.lower()


def reset_snapshot_fixture():
    with connect() as conn:
        conn.execute("DROP TABLE IF EXISTS isolation_counter")
        conn.execute(
            """
            CREATE TABLE isolation_counter (
                id integer PRIMARY KEY,
                value integer NOT NULL
            )
            """
        )
        conn.execute(
            "INSERT INTO isolation_counter (id, value) VALUES (1, 10)"
        )


def snapshot_probe(level):
    reset_snapshot_fixture()

    with connect() as observer, connect() as writer:
        begin(observer, level)
        first = observer.execute(
            "SELECT value FROM isolation_counter WHERE id = 1"
        ).fetchone()[0]

        begin(writer, "READ COMMITTED")
        writer.execute(
            "UPDATE isolation_counter SET value = 20 WHERE id = 1"
        )
        writer.execute("COMMIT")

        second = observer.execute(
            "SELECT value FROM isolation_counter WHERE id = 1"
        ).fetchone()[0]
        observer.execute("ROLLBACK")

    return {"level": level, "first": first, "second": second}


def reset_on_call_fixture():
    with connect() as conn:
        conn.execute("DROP TABLE IF EXISTS isolation_on_call")
        conn.execute(
            """
            CREATE TABLE isolation_on_call (
                engineer text PRIMARY KEY,
                on_call boolean NOT NULL
            )
            """
        )
        conn.execute(
            """
            INSERT INTO isolation_on_call (engineer, on_call)
            VALUES ('ada', true), ('linus', true)
            """
        )


def write_skew_probe(level):
    reset_on_call_fixture()
    reads_complete = threading.Barrier(2)
    results = {}
    results_lock = threading.Lock()

    def go_off_call(engineer):
        outcome = {"engineer": engineer, "sqlstate": None}

        try:
            with connect() as conn:
                begin(conn, level)
                available = conn.execute(
                    """
                    SELECT count(*)
                    FROM isolation_on_call
                    WHERE on_call
                    """
                ).fetchone()[0]
                outcome["available_at_decision"] = available
                assert available == 2

                reads_complete.wait(timeout=5)
                conn.execute(
                    """
                    UPDATE isolation_on_call
                    SET on_call = false
                    WHERE engineer = %s
                    """,
                    (engineer,),
                )
                conn.execute("COMMIT")
                outcome["committed"] = True
        except psycopg.Error as error:
            outcome["committed"] = False
            outcome["sqlstate"] = error.sqlstate

        with results_lock:
            results[engineer] = outcome

    with ThreadPoolExecutor(max_workers=2) as pool:
        futures = [
            pool.submit(go_off_call, "ada"),
            pool.submit(go_off_call, "linus"),
        ]
        for future in futures:
            future.result(timeout=10)

    with connect() as conn:
        remaining = conn.execute(
            "SELECT count(*) FROM isolation_on_call WHERE on_call"
        ).fetchone()[0]

    return {
        "level": level,
        "workers": results,
        "remaining_on_call": remaining,
    }


def main():
    snapshot_read_committed = snapshot_probe("READ COMMITTED")
    snapshot_repeatable_read = snapshot_probe("REPEATABLE READ")
    skew_repeatable_read = write_skew_probe("REPEATABLE READ")
    skew_serializable = write_skew_probe("SERIALIZABLE")

    assert snapshot_read_committed == {
        "level": "READ COMMITTED",
        "first": 10,
        "second": 20,
    }
    assert snapshot_repeatable_read == {
        "level": "REPEATABLE READ",
        "first": 10,
        "second": 10,
    }

    rr_workers = list(skew_repeatable_read["workers"].values())
    assert all(worker["committed"] for worker in rr_workers)
    assert skew_repeatable_read["remaining_on_call"] == 0

    serial_workers = list(skew_serializable["workers"].values())
    assert sum(worker["committed"] for worker in serial_workers) == 1
    assert sorted(
        worker["sqlstate"]
        for worker in serial_workers
        if worker["sqlstate"] is not None
    ) == ["40001"]
    assert skew_serializable["remaining_on_call"] == 1

    print(
        json.dumps(
            {
                "snapshot_read_committed": snapshot_read_committed,
                "snapshot_repeatable_read": snapshot_repeatable_read,
                "skew_repeatable_read": skew_repeatable_read,
                "skew_serializable": skew_serializable,
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
```

The SQL isolation keyword is interpolated only after checking it against a closed set. Do not put user input into that statement. Values in normal queries remain driver parameters.

The snapshot probe uses a deliberate commit boundary rather than a timing race. The write-skew probe uses a barrier only after both transactions have read the invariant. Neither assertion depends on one worker sleeping long enough for the other to reach a guessed line.

## 2. Run the proof in a disposable Postgres database

Run both schedules inside one supervised child process:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint transaction-isolation-proof \
  --ttl-minutes 15 \
  --cleanup always \
  --timeout-seconds 45 \
  --result-format json \
  -- uv run --with 'psycopg[binary]' \
    python tests/postgres_isolation_proof.py
```

`pgsandbox with-database` creates a tracked database and restricted login role, injects `PGSANDBOX_DATABASE_URL` and standard connection variables into the child, supervises its timeout and output, then applies the selected cleanup policy. The repository's [agent test-session documentation](https://github.com/LVTD-LLC/pgsandbox/blob/main/docs/agent-testing.md) defines the structured child and cleanup result.

One child process is important. Separate MCP `run_sql` calls do not preserve two open physical sessions across requests. The same process boundary appears in the [disposable Postgres integration-test guide](/blog/run-integration-tests-disposable-postgres-database/), which explains how to keep repository setup, test execution, and cleanup in one bounded session.

Use `--cleanup always` for CI and unattended coding-agent work. The sandbox TTL is a recovery backstop, not the primary cleanup mechanism. If you retain a failed sandbox for diagnosis, keep its safe database ID and never print its connection URL.

## 3. Interpret the isolation proof

The expected result is a four-row behavior matrix:

| Probe | Isolation level | Expected database result | What it proves |
| --- | --- | --- | --- |
| Repeated scalar read | Read Committed | `10`, then `20` | Each statement can take a newer committed snapshot |
| Repeated scalar read | Repeatable Read | `10`, then `10` | The transaction keeps its initial snapshot |
| On-call write skew | Repeatable Read | Both commit; zero rows remain on call | Stable snapshots do not guarantee a serializable business outcome |
| On-call write skew | Serializable | One commit; one `40001`; one row remains on call | PostgreSQL rejects the unsafe dependency pattern |

The last two rows carry the information that a simple nonrepeatable-read demo misses. Repeatable Read is doing exactly what it promises: each transaction sees a stable snapshot containing two on-call engineers. The problem is that both transactions act on compatible old snapshots and update different rows. Serializable adds the dependency check needed to protect the cross-row invariant.

Do not turn "one worker aborts" into a fixed worker assertion. PostgreSQL can choose either transaction as the serialization failure. Assert one commit, one `40001`, and the final invariant.

The harness intentionally does not retry the Serializable loser. This test isolates level behavior. Application retry belongs in a separate test that proves the entire unit of work is replayed, as shown in the [Postgres serialization failure retry guide](/blog/test-postgres-serialization-failure-retries/).

## 4. Choose isolation from the application invariant

Start with the invariant, then select the weakest mechanism that proves it under concurrency.

- Use **Read Committed** when each statement may use the newest committed snapshot and correctness rests on row constraints, atomic updates, or explicit row locks.
- Use **Repeatable Read** when one transaction needs a stable snapshot and the application can tolerate or otherwise prevent write skew.
- Use **Serializable** when multi-row or predicate-based decisions must behave like one-at-a-time execution and the application has a bounded whole-transaction retry path.
- Treat **Read Uncommitted** as Read Committed in PostgreSQL. Requesting the weaker SQL name does not enable dirty reads.

Higher isolation is not a substitute for constraints. Unique, foreign-key, exclusion, and check constraints should still encode invariants that PostgreSQL can enforce directly. Explicit locking can also be correct when the protected rows are known and the blocking policy is intentional.

Serializable is likewise not "no concurrency." PostgreSQL uses Serializable Snapshot Isolation and aborts dangerous dependency patterns instead of forcing every transaction into a global queue. Its documentation recommends short transactions, controlled connection counts, read-only declarations where applicable, and a general `40001` retry path.

For a coding agent, record the choice next to the observed schedule: "Serializable is required because two transactions update different rows after reading one shared predicate" is reviewable. "Serializable is safer" is not.

## 5. Keep isolation tests deterministic

Isolation tests become flaky when timing is the hidden coordinator. Use database-visible boundaries instead.

1. Open distinct physical connections. Two cursors on one connection cannot run independent top-level transactions.
2. Set isolation before the first query. Verify it with `SHOW transaction_isolation`.
3. Synchronize after the load-bearing reads, not before transaction start.
4. Put finite server timeouts inside each transaction and an outer process timeout around the harness.
5. Match SQLSTATE, not localized exception text.
6. Assert final business state from a fresh connection after workers finish.
7. Close every connection even when a worker fails.

Do not reuse a pooled connection unless the test proves it received distinct backend sessions. The [connection-pool failure guide](/blog/test-postgres-connection-pool-failures/) covers pool acquisition, backend termination, replacement, and recovery as a separate contract.

Do not combine deadlock, lock timeout, serialization failure, and query cancellation into one generic "concurrency error" case. The [deadlock and lock-timeout guide](/blog/test-postgres-deadlocks-lock-timeouts/) distinguishes SQLSTATE `40P01` from `55P03`; each condition protects a different boundary.

### What this harness does not prove

The proof covers one deterministic snapshot change and one two-row write-skew schedule. It does not prove every query in an application is serializable, measure production contention, validate a retry budget, or model external side effects such as payments and webhooks.

Use production-like transaction code in a separate integration test. Keep retry policy deterministic in unit tests. If a retried transaction emits external effects, move them after commit or use an outbox with a stable idempotency key.

## PR-ready isolation proof

A concise review artifact should include:

```text
PostgreSQL isolation proof
- server: PostgreSQL 18 disposable sandbox
- snapshot probe:
  - Read Committed: 10 -> 20
  - Repeatable Read: 10 -> 10
- write-skew probe:
  - Repeatable Read: 2 commits, 0 on call
  - Serializable: 1 commit, 1 x SQLSTATE 40001, 1 on call
- transaction level verified inside each transaction
- child exit code: 0; timedOut: false
- cleanup policy: always; deleted: true
```

Keep credentials out of the artifact. The isolation level, fixed observations, SQLSTATE, final invariant, child status, and cleanup result are enough for review.

## Frequently asked questions

### Which transaction isolation level is PostgreSQL's default?

Read Committed is PostgreSQL's default transaction isolation level. Each statement sees data committed before that statement began, so two statements in one transaction can observe different concurrent commits. Check the actual level inside a test with `SHOW transaction_isolation`.

### Does PostgreSQL support Read Uncommitted?

PostgreSQL accepts the `READ UNCOMMITTED` name but implements it as Read Committed. Dirty reads are not exposed. A test should expect the same statement-snapshot behavior from both requested names.

### Is Repeatable Read serializable in PostgreSQL?

No. PostgreSQL Repeatable Read gives a stable transaction snapshot and prevents phantom reads, but it can still allow serialization anomalies. A write-skew schedule can let two Repeatable Read transactions commit a final state that no serial order would produce.

### How do you test PostgreSQL isolation levels?

Use at least two physical connections, select isolation before the first query, coordinate the exact concurrent schedule, assert driver SQLSTATE values, and query final state from a fresh connection. Run the proof in a disposable database so deliberate conflicts do not affect shared state.

### Should an isolation test use `sleep()`?

No sleep should carry the correctness proof. Use barriers, committed transaction boundaries, locks, or database-visible state to establish ordering. Timeouts should cap failure duration, not decide which statement wins a race.

### Should a Serializable test retry SQLSTATE 40001?

Separate the concerns. One test should prove PostgreSQL rejects the unsafe schedule with `40001`. Another should prove the application rolls back and replays the complete transaction from a fresh snapshot with a bounded retry policy.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "How to Test PostgreSQL Transaction Isolation Levels",
      "description": "Prove statement snapshots, stable transaction snapshots, and Serializable write-skew protection with controlled concurrent PostgreSQL transactions.",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Create a known fixture", "text": "Create fixed snapshot and on-call fixtures in a disposable PostgreSQL database."},
        {"@type": "HowToStep", "position": 2, "name": "Open physical connections", "text": "Use separate database sessions and choose isolation before each transaction's first query."},
        {"@type": "HowToStep", "position": 3, "name": "Run the snapshot probe", "text": "Commit an update between two reads and compare Read Committed with Repeatable Read."},
        {"@type": "HowToStep", "position": 4, "name": "Run the write-skew probe", "text": "Coordinate two predicate reads and distinct-row updates under Repeatable Read and Serializable."},
        {"@type": "HowToStep", "position": 5, "name": "Verify and clean up", "text": "Assert SQLSTATE 40001 and final state, close connections, and delete the disposable database."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "Which transaction isolation level is PostgreSQL's default?", "acceptedAnswer": {"@type": "Answer", "text": "Read Committed is PostgreSQL's default. Each statement can see a newer committed snapshot than an earlier statement in the same transaction."}},
        {"@type": "Question", "name": "Does PostgreSQL support Read Uncommitted?", "acceptedAnswer": {"@type": "Answer", "text": "PostgreSQL accepts the Read Uncommitted name but implements it with Read Committed behavior, so dirty reads are not exposed."}},
        {"@type": "Question", "name": "Is Repeatable Read serializable in PostgreSQL?", "acceptedAnswer": {"@type": "Answer", "text": "No. PostgreSQL Repeatable Read provides a stable snapshot and prevents phantom reads, but it can still allow serialization anomalies such as write skew."}},
        {"@type": "Question", "name": "How do you test PostgreSQL isolation levels?", "acceptedAnswer": {"@type": "Answer", "text": "Use separate physical connections, select isolation before the first query, coordinate a deterministic schedule, assert SQLSTATE and final state, and clean up the disposable database."}},
        {"@type": "Question", "name": "Should an isolation test use sleep?", "acceptedAnswer": {"@type": "Answer", "text": "No sleep should establish correctness. Use barriers or committed database boundaries for ordering, and use timeouts only to bound failures."}},
        {"@type": "Question", "name": "Should a Serializable test retry SQLSTATE 40001?", "acceptedAnswer": {"@type": "Answer", "text": "Use one test to prove the database abort and a separate test to prove bounded whole-transaction replay from a fresh snapshot."}}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test PostgreSQL Transaction Isolation Levels", "item": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-transaction-isolation-levels/"}
      ]
    }
  ]
}
</script>
