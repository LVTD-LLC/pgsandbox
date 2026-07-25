---
title: "How to Test Postgres Serialization Failures and Retries"
excerpt: "Force SQLSTATE 40001 with two coordinated transactions, retry the complete unit of work, and prove the final state in disposable Postgres."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-25"
updatedAt: "2026-07-25T06:00:00Z"
tags: ["Postgres", "serialization failures", "transaction retries", "integration testing", "coding agents"]
category: "Engineering"
metaTitle: "Test Postgres Serialization Failures and Retries"
metaDescription: "Test Postgres SQLSTATE 40001 with coordinated transactions, whole-transaction retries, final-state assertions, and disposable cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-serialization-failure-retries/"
heroImageUrl: ""
featured: false
sortOrder: 147
---
When PostgreSQL returns SQLSTATE `40001`, retry the complete transaction from a fresh snapshot. Catch the error around the whole unit of work, roll back, rerun every decision-making read and write, and trust the result only after commit. Retrying the last statement can reuse a decision made from the aborted transaction's stale view.

The test should prove more than "the second attempt passed." It should force a real conflict, identify `40001` without matching English text, show that one initial attempt was rolled back, replay the losing operation, and assert the final business state. PGSandbox MCP's `pgsandbox with-database` command supplies a local disposable database lifecycle so deliberate concurrency does not interfere with shared development work.

This guide packages the evidence into a **Serialization Retry Proof Contract** with five fields: conflict, boundary, classification, replay, and cleanup.

## In this guide

- [Understand what SQLSTATE 40001 means](#what-sqlstate-40001-means)
- [Use the Serialization Retry Proof Contract](#the-serialization-retry-proof-contract)
- [Create a deterministic two-worker harness](#1-create-a-deterministic-serialization-failure-harness)
- [Retry the whole transaction](#2-put-the-retry-boundary-around-the-whole-transaction)
- [Run the proof with PGSandbox](#3-run-the-proof-in-a-disposable-postgres-database)
- [Assert the retry result](#4-assert-the-conflict-replay-and-final-state)
- [Keep external side effects safe](#5-keep-retried-side-effects-idempotent)
- [Diagnose recurring serialization failures](#6-diagnose-retry-exhaustion)
- [Record PR-ready evidence](#pr-ready-serialization-retry-proof)

## What SQLSTATE `40001` means

PostgreSQL uses SQLSTATE `40001`, named `serialization_failure`, when it aborts a transaction whose concurrent result cannot be accepted at the selected isolation level. The current [serialization-failure handling documentation](https://www.postgresql.org/docs/current/mvcc-serialization-failure-handling.html) says both Repeatable Read and Serializable transactions can produce this code.

The message text is not the contract. PostgreSQL can report a concurrent update, concurrent delete, or read/write dependencies among transactions. The documented stable field is `40001`, so application code and tests should inspect the driver's SQLSTATE property. PostgreSQL's [error-code appendix](https://www.postgresql.org/docs/current/errcodes-appendix.html) is the source of truth for the mapping.

Repeatable Read and Serializable can reach `40001` through different conflict shapes:

| Isolation level | Example conflict | What PostgreSQL protects |
| --- | --- | --- |
| Repeatable Read | A transaction tries to update a row changed after its snapshot began | The transaction-level snapshot cannot be reconciled with the concurrent row change |
| Serializable | Concurrent reads and writes form dependencies inconsistent with every serial execution | Committed transactions must match some one-at-a-time order |

PostgreSQL's [transaction-isolation documentation](https://www.postgresql.org/docs/current/transaction-iso.html#XACT-SERIALIZABLE) describes Serializable as Repeatable Read plus monitoring for dangerous read/write dependency combinations. That monitoring does not add blocking beyond Repeatable Read. When PostgreSQL detects an unsafe combination, it aborts a transaction instead.

This matters in a test because the failure can arrive at a statement such as `UPDATE` or at `COMMIT`. Put the exception boundary around the entire transaction context. Do not assume the line that raises the driver exception is the only work that must be repeated.

### Do not merge `40001` with every retryable-looking error

Preserve the database condition:

| SQLSTATE | Condition | Default test expectation |
| --- | --- | --- |
| `40001` | `serialization_failure` | Roll back and replay the complete transaction |
| `40P01` | `deadlock_detected` | Often replayable, but test as a separate conflict |
| `55P03` | `lock_not_available` | A lock budget or `NOWAIT` path; decide from application policy |
| `57014` | `query_canceled` | A timeout or cancellation path, not proof of serialization |
| `23505` | `unique_violation` | Usually a business or concurrency decision, not an unconditional retry |

The official serialization guidance says some deadlock, unique-key, and exclusion-constraint failures may also merit replay, but the last two need case-specific analysis. A blanket "retry every database error" loop can repeat permanent failures and hide a broken invariant. The [Postgres MCP error-handling guide](/blog/postgres-mcp-server-error-handling-coding-agents/) applies the same classification rule to agent tool responses.

## The Serialization Retry Proof Contract

A useful integration test should answer five review questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Conflict | Did both transactions make a decision from the intended concurrent state? | Two independent connections and a barrier after both first reads |
| Boundary | Which code is rerun after failure? | One transaction callback containing isolation, reads, decisions, writes, and commit |
| Classification | Did PostgreSQL return the expected condition? | Driver SQLSTATE `40001`, not an English message match |
| Replay | Did a fresh attempt recompute the decision and commit once? | One worker uses one attempt, one uses two, final value is correct |
| Cleanup | Did the test release connections and remove the sandbox? | Closed contexts plus structured PGSandbox cleanup result |

The contract turns a retry branch into reviewable evidence. It proves that the branch begins at the correct boundary, runs against a new snapshot, produces the intended logical outcome, and leaves no test database behind.

PGSandbox supplies the lifecycle boundary, not the retry algorithm. The repository test process owns both concurrent connections and the application retry policy. PostgreSQL owns isolation and conflict detection. PGSandbox creates the task database and scoped role, injects the connection variables, supervises the child, and applies cleanup.

## 1. Create a deterministic serialization-failure harness

The following Psycopg 3 harness starts with one counter at zero. Two Serializable transactions read zero before either can write. A barrier proves both first snapshots exist. Once both workers pass that barrier, the same-row conflict yields one initial `40001`; unrelated provisioning, connectivity, or timeout failures remain separate test failures. PostgreSQL can choose either worker as the loser.

This fixture tests the replay contract with a same-row conflict that can also occur under Repeatable Read. It does not claim to exercise a Serializable-only write-skew or predicate-lock dependency. The initial winner commits `1`; the loser opens a fresh transaction, reads `1`, and commits `2`.

Save it as `tests/postgres_serialization_retry_proof.py`:

```python
import json
import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]
MAX_ATTEMPTS = 3


def reset_fixture():
    with psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    ) as conn:
        conn.execute("DROP TABLE IF EXISTS serialization_retry_counter")
        conn.execute(
            """
            CREATE TABLE serialization_retry_counter (
                id integer PRIMARY KEY,
                value integer NOT NULL
            )
            """
        )
        conn.execute(
            "INSERT INTO serialization_retry_counter (id, value) VALUES (1, 0)"
        )


def increment(worker, first_read_barrier):
    sqlstates = []
    initial_read = None

    for attempt in range(1, MAX_ATTEMPTS + 1):
        try:
            with psycopg.connect(
                DATABASE_URL,
                autocommit=True,
                connect_timeout=5,
            ) as conn:
                with conn.transaction():
                    conn.execute(
                        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"
                    )
                    conn.execute("SET LOCAL statement_timeout = '5s'")
                    value = conn.execute(
                        """
                        SELECT value
                        FROM serialization_retry_counter
                        WHERE id = 1
                        """
                    ).fetchone()[0]

                    if attempt == 1:
                        initial_read = value
                        first_read_barrier.wait(timeout=5)

                    conn.execute(
                        """
                        UPDATE serialization_retry_counter
                        SET value = %s
                        WHERE id = 1
                        """,
                        (value + 1,),
                    )

            return {
                "worker": worker,
                "attempts": attempt,
                "initialRead": initial_read,
                "sqlstates": sqlstates,
            }
        except errors.SerializationFailure as exc:
            sqlstates.append(exc.sqlstate)
            if attempt == MAX_ATTEMPTS:
                raise
            time.sleep(0.05 * (2 ** (attempt - 1)))

    raise AssertionError("retry loop exhausted")


def main():
    reset_fixture()
    barrier = threading.Barrier(2)

    with ThreadPoolExecutor(max_workers=2) as pool:
        futures = (
            pool.submit(increment, "a", barrier),
            pool.submit(increment, "b", barrier),
        )
        outcomes = [future.result(timeout=15) for future in futures]

    assert [row["initialRead"] for row in outcomes] == [0, 0], outcomes
    assert sorted(row["attempts"] for row in outcomes) == [1, 2], outcomes
    assert sorted(row["sqlstates"] for row in outcomes) == [
        [],
        ["40001"],
    ], outcomes

    with psycopg.connect(DATABASE_URL, connect_timeout=5) as conn:
        final_value = conn.execute(
            """
            SELECT value
            FROM serialization_retry_counter
            WHERE id = 1
            """
        ).fetchone()[0]

    assert final_value == 2, {
        "finalValue": final_value,
        "outcomes": outcomes,
    }
    print(
        json.dumps(
            {
                "committedWorkers": 2,
                "finalValue": final_value,
                "retrySqlstates": ["40001"],
                "serializationFailures": 1,
                "totalAttempts": sum(
                    row["attempts"] for row in outcomes
                ),
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(
            json.dumps(
                {
                    "status": "failed",
                    "errorType": type(exc).__name__,
                },
                sort_keys=True,
            )
        )
        raise SystemExit(1) from None
```

This harness uses coordination, not sleep, to create the conflict. Sleep appears only in the retry policy after PostgreSQL has already returned `40001`. A sleep-only conflict fixture cannot prove that both transactions read the old value before either writes.

Do not assert which worker loses. PostgreSQL's public contract is the serializable outcome and the failure code, not a stable victim identity. The proof requires one first-attempt commit, one serialization failure, one successful replay, and final value `2`.

The child should print this fixed, credential-free proof:

```json
{
  "committedWorkers": 2,
  "finalValue": 2,
  "retrySqlstates": ["40001"],
  "serializationFailures": 1,
  "totalAttempts": 3
}
```

## 2. Put the retry boundary around the whole transaction

The `try` block begins before the connection and transaction contexts and ends after the transaction context commits. That shape catches a failure raised by a statement or by commit. On `40001`, both contexts exit, the failed transaction rolls back, and the next loop iteration opens a new connection and transaction.

The [PostgreSQL serialization-failure guidance](https://www.postgresql.org/docs/current/mvcc-serialization-failure-handling.html) requires replaying the complete transaction, including application logic that decides which SQL to issue and which values to use. PostgreSQL cannot safely retry that work automatically because the database does not own those decisions.

The unsafe shape retries only the statement that happened to raise:

```python
value = read_from_old_transaction()

try:
    update_using(value)
except errors.SerializationFailure:
    update_using(value)
```

The safe shape reruns the operation that owns the reads and writes:

```python
for attempt in range(MAX_ATTEMPTS):
    try:
        return run_complete_transaction()
    except errors.SerializationFailure:
        if attempt + 1 == MAX_ATTEMPTS:
            raise
        wait_before_next_attempt(attempt)
```

In the harness, `value` is read inside every transaction attempt. The loser does not keep its initial zero. It opens a fresh transaction, observes the winner's committed value, and computes the next value from that snapshot. If a new transaction reused the original zero, both logical operations could finish with a final value of `1`; the final-state assertion catches that lost decision.

### Keep retry policy separate from retry correctness

PostgreSQL does not prescribe one universal retry count or backoff formula. Its documentation warns that a replay can fail again under heavy contention. Choose a bounded attempt count and delay policy from the application's latency budget, then observe exhausted retries as a contention signal. The harness uses a fixed exponential delay to keep the proof readable and deterministic; production policy may add capped jitter to prevent workers from repeatedly colliding.

Keep two test layers:

- The real-Postgres integration proof forces one `40001` and verifies the driver, transaction boundary, replay, and final state.
- Fast unit tests inject repeated serialization errors to cover attempt limits, backoff, jitter, cancellation, logging, and the final surfaced error.

Do not make five timing-policy cases depend on a live concurrency race. Use the database once to prove the boundary and use deterministic unit tests to prove the policy.

## 3. Run the proof in a disposable Postgres database

Run the harness as one supervised child process:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint serialization-retry-proof \
  --ttl-minutes 15 \
  --cleanup always \
  --timeout-seconds 45 \
  --result-format json \
  -- uv run --with 'psycopg[binary]' \
    python tests/postgres_serialization_retry_proof.py
```

`pgsandbox with-database` creates a restricted-role database, then injects `DATABASE_URL`, `PGSANDBOX_DATABASE_URL`, and standard libpq variables into the child. The current [agent test-session documentation](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/36703a124c3761f863a8342f1e2a52bd914820b3/docs/agent-testing.md) defines the command's child status, bounded output, timeout, and cleanup result.

The child owns both connections, which is necessary for the barrier and open transactions. Separate PGSandbox `run_sql` calls are bounded request/response operations and do not preserve two transaction sessions across calls. The [deadlock and lock-timeout guide](/blog/test-postgres-deadlocks-lock-timeouts/) uses the same one-child-process boundary for a different concurrency condition.

Use `--cleanup always` for CI and unattended coding-agent work. Use `--cleanup on-success` only during active debugging when a retained failure will actually be inspected and then deleted or allowed to expire. The [sandbox TTL guide](/blog/postgres-sandbox-ttl-values/) explains how to size that recovery window.

Do not print the injected URL, `PGPASSWORD`, derived DSNs, or driver exception strings that may contain connection details. The harness emits a fixed JSON proof with counts and SQLSTATE only. PGSandbox's structured result carries safe lifecycle evidence.

The child proof appears inside `command.stdout` in the version 1 session result. Verify the wrapper fields separately:

```json
{
  "status": "succeeded",
  "command": {
    "exitCode": 0,
    "timedOut": false
  },
  "cleanup": {
    "policy": "always",
    "attempted": true,
    "deleted": true,
    "retained": false
  }
}
```

If `cleanup.deleted` is false, fail or flag the CI job, retain the safe database ID for diagnosis, and let the TTL remain the recovery backstop. Never copy the connection string into the artifact.

## 4. Assert the conflict, replay, and final state

A passing proof needs all of these assertions:

1. Both workers initially read zero. This proves the test created overlapping decisions.
2. One worker finishes in one attempt and one finishes in two. This proves a replay occurred.
3. Exactly one caught SQLSTATE is `40001`. This proves PostgreSQL, not a mock or application timeout, rejected the initial concurrent result.
4. Both logical operations commit. This proves the retry completed.
5. The final counter is two. This proves the loser recomputed its write from a fresh snapshot.
6. The session cleanup result reports the database deleted under the selected policy.

The final-state assertion is load-bearing. If both workers reused zero and wrote one, the test could report two "successful" operations while losing one increment. A correct retry proof verifies the business invariant after concurrency, not only the absence of an exception.

PostgreSQL's Serializable guarantee applies to committed transactions. Treat results from an aborted attempt as unusable. The transaction-isolation documentation explicitly says applications must not rely on results read by a transaction that later aborts.

### Do not expect gapless sequence values

PostgreSQL documents that sequence changes are immediately visible and are not rolled back when the surrounding transaction aborts. If a retried transaction calls `nextval`, the retry may consume another value. Test uniqueness and the business invariant; do not require gapless IDs or expect a failed attempt's sequence value to be reused.

This is one reason the fixture uses an explicit row key and counter. It keeps the serialization assertion separate from sequence behavior.

## 5. Keep retried side effects idempotent

PostgreSQL [`ROLLBACK`](https://www.postgresql.org/docs/current/sql-rollback.html) discards database updates made by the losing transaction. External effects sit outside that rollback boundary. An HTTP request, email, queue publication, file write, or payment call performed inside a callback may happen again when the callback is replayed.

Keep irreversible effects after a successful commit when the workflow allows it. When an effect must be coordinated with database state, use a transactional outbox or another design that records the intent in the database and delivers it with a stable idempotency key.

A retry-aware test suite should cover:

- The database operation commits once logically after a replay.
- The external effect is absent before commit.
- The committed outbox or effect key is unique for the logical operation.
- A repeated delivery attempt does not duplicate the downstream outcome.
- Retry exhaustion surfaces a clear failure instead of silently dropping the operation.

Do not claim that `40001` makes arbitrary code safe to replay. It tells you the database transaction was aborted for serialization. The application still owns the replay boundary and side-effect contract.

## 6. Diagnose retry exhaustion

Retries are a recovery path, not a substitute for diagnosing contention. When the bounded policy is exhausted, record the stable code, operation name, attempt count, elapsed budget, isolation level, and a credential-safe diagnostic handle.

Then inspect the workload:

- Are transactions doing network calls or other slow work between the first read and commit?
- Do many workers update the same row or small key range?
- Is a retry reusing data calculated outside the transaction?
- Is the effective isolation level what the application expects?
- Can one atomic SQL statement replace a read-modify-write sequence?
- Would an explicit row lock make the intended order clearer for this operation?
- Are indexes and query plans broadening the rows or predicate-lock footprint?
- Is application concurrency above the database's useful capacity?

PostgreSQL's Serializable documentation discusses keeping transactions short, controlling active connections, and using appropriate indexes to reduce serialization failures. Lowering isolation can remove `40001` while reintroducing the anomaly Serializable was selected to prevent. Treat that as a correctness decision, not a retry tweak.

If the test produces `40P01`, `55P03`, or `57014` instead, stop calling it a serialization retry proof. Use the [two-connection concurrency guide](/blog/test-postgres-deadlocks-lock-timeouts/) to verify the intended topology and classify the actual failure.

### When not to use this retry pattern

Do not add a Serializable retry wrapper to every database operation. A single atomic statement at Read Committed may already express the correct invariant. A job queue built around `FOR UPDATE SKIP LOCKED` has a different concurrency contract. An operation with external effects that cannot be deferred, deduplicated, or made idempotent is not safe to replay as one callback.

## Common serialization retry testing mistakes

### Retrying only the failed statement

The earlier reads and decisions belong to the failed snapshot. Roll back and replay the complete transaction.

### Catching outside the wrong boundary

A Serializable failure may surface at commit. Catch `40001` around the transaction context, not only around `UPDATE`.

### Matching error text

Message text varies with the conflict and localization. Assert the driver's SQLSTATE field equals `40001`.

### Coordinating with sleep

Sleep guesses at scheduling. Put a barrier after both decision-making reads so the test proves the snapshots overlap.

### Asserting a specific loser

PostgreSQL does not promise worker A or worker B as the victim. Assert one initial success and one replay.

### Running against shared development data

The test deliberately creates overlapping transactions and aborts one. Run it in a task-scoped [database sandbox](/blog/what-is-database-sandbox/) with a scoped role, TTL, and cleanup.

### Printing the database URL

Connection URLs contain credentials. Print fixed proof fields and use the safe sandbox identifier from the structured session result.

## PR-ready serialization retry proof

Record a compact result with a transaction-retry patch:

```text
Postgres serialization retry proof
- Target: PostgreSQL=<major>, sandbox=<safe database ID>
- Isolation: SERIALIZABLE
- Conflict: two workers read value=0 before either UPDATE
- Classification: one SQLSTATE 40001 on initial attempts
- Replay: attempts=[1,2], complete transaction rerun
- Invariant: committed workers=2, final value=2
- Side effects: post-commit or idempotency/outbox test=<result>
- Session: status=<status>, child exit=<code>, elapsed=<duration>
- Cleanup: policy=always, deleted=<yes/no>, error=<stable code or none>
```

This proof is useful in review because it states the overlapping decision, transaction boundary, database condition, logical result, and cleanup without including credentials or unbounded logs.

Install PGSandbox from the [setup guide](/docs/install/), then run the proof through the [one-shot database test workflow](/blog/run-integration-tests-disposable-postgres-database/) with your repository's actual transaction callback.

## Frequently asked questions

### What causes a PostgreSQL serialization failure?

PostgreSQL raises SQLSTATE `40001` when it cannot accept a concurrent transaction result at Repeatable Read or Serializable isolation. The cause may be a concurrent row change or a Serializable read/write dependency pattern that cannot match any serial execution.

### How do you force a PostgreSQL serialization failure in a test?

Open two Serializable transactions, make both read the same initial row value, and synchronize them with a barrier before either writes. Let both update from that shared initial state. Assert that one initial attempt commits, one receives SQLSTATE `40001`, and the losing operation succeeds after a complete replay.

### Should you retry the query or the whole transaction?

Retry the complete transaction from the beginning, including every read and application decision that selects later SQL or values. The failed transaction's snapshot is no longer valid, so retrying only the statement can repeat a decision made from stale state.

### Can PostgreSQL retry `40001` automatically?

No general automatic retry can be guaranteed correct. PostgreSQL does not own the application logic that chose the statements and values, so the application must define a safe replay boundary and handle external side effects.

### What is the difference between `40001` and `40P01`?

`40001` is `serialization_failure`: PostgreSQL rejected a concurrent outcome at Repeatable Read or Serializable isolation. `40P01` is `deadlock_detected`: transactions formed a lock-wait cycle. Both may justify full-transaction replay, but they are different conditions and need separate tests.

### How many times should an application retry a serialization failure?

PostgreSQL does not specify one retry count. Use a bounded policy chosen from the operation's latency budget, add a delay or jitter policy when appropriate, observe exhausted retries, and reduce the underlying contention rather than retrying forever.

### How do retries affect emails, webhooks, and payments?

PostgreSQL rolls back database writes from the failed transaction, but it cannot retract external effects. Perform them after commit or coordinate them through an outbox and stable idempotency key so replay does not duplicate the logical action.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Test Postgres serialization failures and retries",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Create overlapping snapshots", "text": "Open two Serializable transactions and synchronize them after both read the same initial value."},
        {"@type": "HowToStep", "position": 2, "name": "Force SQLSTATE 40001", "text": "Let both transactions write from their initial decisions and assert exactly one serialization failure."},
        {"@type": "HowToStep", "position": 3, "name": "Replay the complete transaction", "text": "Roll back the loser, open a fresh transaction, rerun its reads and writes, and trust the result only after commit."},
        {"@type": "HowToStep", "position": 4, "name": "Verify the final invariant", "text": "Require two committed logical operations, one retry, one captured 40001, and final counter value two."},
        {"@type": "HowToStep", "position": 5, "name": "Clean up the database", "text": "Run the proof with pgsandbox with-database using a bounded timeout, structured result, and explicit cleanup policy."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "What causes a PostgreSQL serialization failure?", "acceptedAnswer": {"@type": "Answer", "text": "PostgreSQL raises SQLSTATE 40001 when it cannot accept a concurrent transaction result at Repeatable Read or Serializable isolation."}},
        {"@type": "Question", "name": "How do you force a PostgreSQL serialization failure in a test?", "acceptedAnswer": {"@type": "Answer", "text": "Open two Serializable transactions, synchronize them after both read the same initial value, then let both write. Assert one initial commit, one SQLSTATE 40001, and one complete replay."}},
        {"@type": "Question", "name": "Should you retry the query or the whole transaction?", "acceptedAnswer": {"@type": "Answer", "text": "Retry the complete transaction, including every read and application decision. Retrying only the statement can reuse stale state from the aborted snapshot."}},
        {"@type": "Question", "name": "Can PostgreSQL retry 40001 automatically?", "acceptedAnswer": {"@type": "Answer", "text": "No general automatic retry is guaranteed correct because PostgreSQL does not own the application logic that chose the transaction's statements and values."}},
        {"@type": "Question", "name": "What is the difference between 40001 and 40P01?", "acceptedAnswer": {"@type": "Answer", "text": "40001 is a serialization failure. 40P01 is a deadlock detected from a lock-wait cycle. They are different conditions and need separate tests."}},
        {"@type": "Question", "name": "How many times should an application retry a serialization failure?", "acceptedAnswer": {"@type": "Answer", "text": "PostgreSQL does not specify one count. Use a bounded policy chosen from the operation's latency budget and diagnose contention when retries are exhausted."}},
        {"@type": "Question", "name": "How do retries affect emails, webhooks, and payments?", "acceptedAnswer": {"@type": "Answer", "text": "PostgreSQL rolls back database writes but cannot retract external effects. Perform them after commit or use an outbox and stable idempotency key."}}
      ]
    }
  ]
}
</script>
