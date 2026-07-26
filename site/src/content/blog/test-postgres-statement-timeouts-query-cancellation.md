---
title: "How to Test Postgres Statement Timeouts and Query Cancellation"
excerpt: "Force SQLSTATE 57014 with a server timeout and an explicit client cancel, prove transaction recovery, and keep the test inside disposable Postgres."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-26"
updatedAt: "2026-07-26T06:00:00Z"
tags: ["Postgres", "statement timeout", "query cancellation", "integration testing", "coding agents"]
category: "Engineering"
metaTitle: "Test Postgres Statement Timeouts and Query Cancellation"
metaDescription: "Test Postgres statement_timeout and client cancellation with SQLSTATE 57014, transaction recovery, and a disposable database."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-statement-timeouts-query-cancellation/"
heroImageUrl: ""
featured: false
sortOrder: 148
---
Test Postgres query cancellation as three separate controls: the server's `statement_timeout`, the driver's explicit cancel request, and the outer test-process deadline. Force each database path against real PostgreSQL, classify successful cancellation with SQLSTATE `57014`, then prove whether the connection needs a rollback before reuse.

A useful test must also record which control fired. PostgreSQL maps both a statement timeout and a successful client cancel to `57014` (`query_canceled`). The code alone identifies the database condition, not the origin. Your harness supplies that missing provenance from the control it deliberately activated.

This guide turns that distinction into a **Cancellation Proof Contract**: budget, trigger, classification, recovery, and cleanup. PGSandbox MCP provides the disposable database boundary so a deliberately slow statement does not run against shared development state.

## In this guide

- [Separate the three cancellation clocks](#separate-the-three-cancellation-clocks)
- [Use the Cancellation Proof Contract](#the-cancellation-proof-contract)
- [Create the Psycopg cancellation harness](#1-create-a-deterministic-cancellation-harness)
- [Run it in disposable Postgres](#2-run-the-proof-with-pgsandbox)
- [Prove transaction recovery](#3-prove-the-right-connection-recovery)
- [Distinguish timeout from explicit cancellation](#4-identify-which-cancellation-control-fired)
- [Diagnose failed cancellation tests](#5-diagnose-a-failed-cancellation-proof)
- [Record PR-ready evidence](#pr-ready-cancellation-proof)

## Separate the three cancellation clocks

Postgres statement-timeout testing becomes confusing when one test has several deadlines but reports only "timed out."

| Control | Owner | What it bounds | Expected evidence |
| --- | --- | --- | --- |
| `statement_timeout` | PostgreSQL | One server statement | SQLSTATE `57014`, known configured value, query did not finish |
| Driver cancel | Application or test | The current operation on one connection | Cancel was requested while the target backend was active, then SQLSTATE `57014` if the server applied it |
| Outer process timeout | Test runner or PGSandbox | The complete child process | Process timeout or signal result, not a PostgreSQL SQLSTATE |

PostgreSQL's current [`statement_timeout` reference](https://www.postgresql.org/docs/current/runtime-config-client.html#GUC-STATEMENT-TIMEOUT) says the server measures the timeout from command arrival until command completion. Zero disables it. In the simple-query protocol, PostgreSQL 13 and newer apply the budget separately to each statement in a multi-statement message; older versions usually applied it to the whole query string.

`lock_timeout` is narrower. It applies only while PostgreSQL is trying to acquire a lock, and it uses SQLSTATE `55P03` (`lock_not_available`). A statement can spend time scanning, sorting, executing a function, or waiting for a lock, so `statement_timeout` can end work that has nothing to do with lock acquisition. The [deadlock and lock-timeout testing guide](/blog/test-postgres-deadlocks-lock-timeouts/) covers that separate branch.

The outer process deadline is intentionally larger than either database budget. It catches a broken harness, driver hang, or cancellation request that did not take effect. If the outer deadline fires first, the test has not proved PostgreSQL statement cancellation.

### Why SQLSTATE is necessary but not sufficient

The PostgreSQL [error-code appendix](https://www.postgresql.org/docs/current/errcodes-appendix.html) assigns `57014` to `query_canceled`. That stable code is safer than matching English messages such as "canceling statement due to statement timeout" or "due to user request."

However, `57014` does not tell you who initiated cancellation:

- PostgreSQL can abort the statement when `statement_timeout` expires.
- A driver can send a PostgreSQL `CancelRequest`.
- Another authorized session can call `pg_cancel_backend(pid)`.
- Operational tooling can cancel a backend's current statement.

Record SQLSTATE for classification and record the activated control for provenance. Do not infer the cause from SQLSTATE alone.

## The Cancellation Proof Contract

A reviewable integration test should answer five questions.

| Field | Question | Evidence |
| --- | --- | --- |
| Budget | Which clock was supposed to fire first? | Server budget, cancel deadline, and larger outer-process deadline |
| Trigger | Was the intended operation active before cancellation? | `SHOW statement_timeout` or a `pg_stat_activity` observation of the target PID |
| Classification | Did PostgreSQL report query cancellation? | Driver SQLSTATE `57014`, not message text |
| Recovery | Is the connection in the expected state afterward? | Immediate reuse in autocommit; `25P02` then rollback in an explicit transaction |
| Cleanup | Did the test remove the database and close connections? | Closed connection contexts and a structured PGSandbox cleanup result |

This contract is the information gain over a one-line `SELECT pg_sleep(10)` example. It proves not only that a query stopped, but also that the intended clock stopped it, the application recognized the stable condition, and the connection returned to a known state.

## 1. Create a deterministic cancellation harness

The following Psycopg 3 harness tests three paths:

1. a server-side `statement_timeout` in autocommit mode;
2. an explicit client cancel sent only after an observer sees the target query active;
3. a timeout inside an explicit transaction, followed by the required rollback.

It uses `pg_sleep(10)` as bounded test work. The outer PGSandbox deadline is 30 seconds, so a broken cancellation path cannot hang the job indefinitely.

Save this as `tests/postgres_cancellation_proof.py`:

```python
import json
import os
import time
from concurrent.futures import ThreadPoolExecutor

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]


def reset_fixture():
    with psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    ) as conn:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS cancellation_proof_markers (
                marker text PRIMARY KEY
            )
            """
        )
        conn.execute("TRUNCATE cancellation_proof_markers")


def canceled_sqlstate(exc):
    assert isinstance(exc, errors.QueryCanceled), type(exc).__name__
    assert exc.sqlstate == "57014", exc.sqlstate
    return exc.sqlstate


def prove_statement_timeout():
    with psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    ) as conn:
        conn.execute("SET statement_timeout = '200ms'")
        configured = conn.execute(
            "SHOW statement_timeout"
        ).fetchone()[0]
        assert configured == "200ms", configured

        try:
            conn.execute("SELECT pg_sleep(10)")
        except errors.QueryCanceled as exc:
            sqlstate = canceled_sqlstate(exc)
        else:
            raise AssertionError("statement_timeout did not cancel pg_sleep")

        recovery_value = conn.execute("SELECT 42").fetchone()[0]
        assert recovery_value == 42

    return {
        "control": "statement_timeout",
        "configured": configured,
        "sqlstate": sqlstate,
        "connectionReusable": True,
    }


def wait_until_target_is_active(observer, backend_pid):
    deadline = time.monotonic() + 5

    while time.monotonic() < deadline:
        row = observer.execute(
            """
            SELECT state, query
            FROM pg_stat_activity
            WHERE pid = %s
            """,
            (backend_pid,),
        ).fetchone()

        if (
            row
            and row[0] == "active"
            and row[1]
            and "pgsandbox-client-cancel-proof" in row[1]
        ):
            return

        time.sleep(0.02)

    raise AssertionError("target pg_sleep never became active")


def run_cancel_target(conn):
    try:
        conn.execute(
            "SELECT pg_sleep(10) "
            "/* pgsandbox-client-cancel-proof */"
        )
    except errors.QueryCanceled as exc:
        return canceled_sqlstate(exc)

    raise AssertionError("client cancel did not stop pg_sleep")


def prove_explicit_cancel():
    with (
        psycopg.connect(
            DATABASE_URL,
            autocommit=True,
            connect_timeout=5,
        ) as target,
        psycopg.connect(
            DATABASE_URL,
            autocommit=True,
            connect_timeout=5,
        ) as observer,
        ThreadPoolExecutor(max_workers=1) as pool,
    ):
        target.execute("SET statement_timeout = 0")
        assert target.execute(
            "SHOW statement_timeout"
        ).fetchone()[0] == "0"
        target.execute(
            "SET application_name = 'pgsandbox-cancel-proof'"
        )
        target_pid = target.info.backend_pid
        future = pool.submit(run_cancel_target, target)

        wait_until_target_is_active(observer, target_pid)
        try:
            target.cancel_safe(timeout=5)
        except errors.CancellationTimeout as exc:
            raise AssertionError(
                "cancel request exceeded its client deadline"
            ) from exc
        sqlstate = future.result(timeout=5)

        same_backend_pid = target.info.backend_pid == target_pid
        recovery_value = target.execute("SELECT 42").fetchone()[0]
        assert same_backend_pid
        assert recovery_value == 42

    return {
        "control": "client_cancel_safe",
        "targetObservedActive": True,
        "sqlstate": sqlstate,
        "sameBackendPid": same_backend_pid,
        "connectionReusable": True,
    }


def prove_transaction_recovery():
    with psycopg.connect(
        DATABASE_URL,
        autocommit=False,
        connect_timeout=5,
    ) as conn:
        conn.execute("SET LOCAL statement_timeout = '200ms'")
        conn.execute(
            """
            INSERT INTO cancellation_proof_markers (marker)
            VALUES ('before-cancel')
            """
        )

        try:
            conn.execute("SELECT pg_sleep(10)")
        except errors.QueryCanceled as exc:
            sqlstate = canceled_sqlstate(exc)
        else:
            raise AssertionError("transaction query was not canceled")

        try:
            conn.execute("SELECT 1")
        except errors.InFailedSqlTransaction as exc:
            assert exc.sqlstate == "25P02", exc.sqlstate
            blocked_sqlstate = exc.sqlstate
        else:
            raise AssertionError(
                "failed transaction accepted another statement"
            )

        conn.rollback()
        marker_count = conn.execute(
            """
            SELECT count(*)
            FROM cancellation_proof_markers
            WHERE marker = 'before-cancel'
            """
        ).fetchone()[0]
        recovery_value = conn.execute("SELECT 42").fetchone()[0]
        conn.commit()
        assert marker_count == 0
        assert recovery_value == 42

    return {
        "control": "transaction_statement_timeout",
        "sqlstate": sqlstate,
        "beforeRollback": blocked_sqlstate,
        "rolledBackMarkerCount": marker_count,
        "afterRollbackReusable": True,
    }


def main():
    reset_fixture()
    proof = {
        "statementTimeout": prove_statement_timeout(),
        "explicitCancel": prove_explicit_cancel(),
        "transactionRecovery": prove_transaction_recovery(),
    }
    print(json.dumps(proof, sort_keys=True))


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

### Why the explicit-cancel test observes the backend

A fixed sleep before `cancel_safe()` creates a race. The cancel can arrive before `pg_sleep` starts or after it finishes. PostgreSQL's [cancel-request protocol](https://www.postgresql.org/docs/current/protocol-flow.html#PROTOCOL-FLOW-CANCELING-REQUESTS) explicitly says a cancellation signal might have no effect if it arrives after the backend completes the query.

The harness removes that guess. It gets the target connection's backend PID from the driver, starts the query, and polls `pg_stat_activity` from a second connection using the same scoped sandbox role. Only after the target PID is active on the expected statement does it issue cancellation. It then confirms the same backend PID remains, proving the test canceled a command rather than terminating the session.

The protocol still does not give the cancel sender a direct success reply. A client must wait for the original query response. That is why the test requires the worker to receive `57014`; a successful return from `cancel_safe()` alone is not the assertion.

Psycopg's current [`cancel_safe()` documentation](https://www.psycopg.org/psycopg3/docs/api/connections.html#psycopg.Connection.cancel_safe) makes the same distinction: a successful client-side cancel attempt does not guarantee that PostgreSQL canceled the operation. Psycopg 3.2 added `cancel_safe()`. With libpq 17 or newer it uses the improved non-blocking cancellation API; with older libpq it falls back to the legacy implementation, and the method's timeout parameter has no effect.

Pin Psycopg 3.2 or newer for this exact example. `CancellationTimeout` is a client-side failure to complete the cancel-request operation within its deadline; it has no PostgreSQL SQLSTATE and is not interchangeable with the target query's `QueryCanceled`/`57014` result. If a repository uses another driver, keep the proof contract but replace `cancel_safe()` and the error-property access with that driver's documented equivalents.

## 2. Run the proof with PGSandbox

Run the harness through one task-scoped database:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 15 \
  --cleanup always \
  --timeout-seconds 30 \
  --result-format json \
  -- uv run --with 'psycopg[binary]>=3.2,<4' \
    python tests/postgres_cancellation_proof.py
```

Install and configure PGSandbox first with the [setup guide](/docs/install/). The example selects PostgreSQL 18; use another installed major or an approved profile when that matches the application's support matrix.

PGSandbox creates the database and scoped login role, injects `PGSANDBOX_DATABASE_URL`, supervises the child, captures bounded credential-redacted output, and applies the selected cleanup policy. The [MCP tool contract](/docs/mcp-tools/) and [architecture guide](/docs/architecture/) explain the separation between lifecycle authority and the role used by repository code.

The control boundaries stay explicit:

- PGSandbox owns database, credential, process, TTL, and cleanup lifecycle.
- The repository harness owns the statement budgets and driver cancel call.
- PostgreSQL owns statement execution, cancel processing, SQLSTATE, and transaction state.
- The outer 30-second deadline owns the "test itself is stuck" failure.

Use `--cleanup always` in CI and unattended agent work. `--cleanup on-success` is useful only when a human plans to inspect a failed sandbox before its TTL expires. The [sandbox TTL guide](/blog/postgres-sandbox-ttl-values/) shows how to bound that retention window.

Do not print either injected database URL. The child output above contains only control names, timeout values, SQLSTATEs, and boolean recovery assertions. Use the PGSandbox session result for a safe database identifier and cleanup status.

## 3. Prove the right connection recovery

Cancellation ends the current statement. What happens next depends on the transaction boundary.

### Autocommit: prove immediate reuse

When a statement executes outside an explicit transaction block, PostgreSQL treats it as an implicit transaction. The current [protocol-flow documentation](https://www.postgresql.org/docs/current/protocol-flow.html#PROTOCOL-FLOW-MULTI-STATEMENT) says an implicit transaction block is automatically rolled back when an error occurs.

That is why both autocommit cases run `SELECT 42` on the same connection immediately after catching `57014`. The passing query proves the connection returned to an idle, usable state. Do not replace this with "opening a new connection worked"; that would skip the recovery behavior the application needs.

### Explicit transaction: require rollback

Inside an explicit transaction, a canceled statement leaves the transaction failed. The next statement should return SQLSTATE `25P02` (`in_failed_sql_transaction`) until the application rolls back.

The transaction-recovery case proves this sequence:

1. `SET LOCAL statement_timeout = '200ms'` scopes the budget to the transaction.
2. `pg_sleep(10)` receives `57014`.
3. `SELECT 1` before rollback receives `25P02`.
4. `rollback()` clears the failed transaction and removes the marker inserted before cancellation.
5. `SELECT 42` succeeds on the same connection.

This is an important application contract. Returning a failed connection to a pool without rollback can make a later request fail in code that appears unrelated to the original cancellation.

Do not automatically retry every canceled statement. A read may be safe to retry after narrowing its scope or increasing a justified budget. A canceled write may have triggers, external side effects, or an unknown completion boundary at another layer. Recovery policy belongs to the operation, not to SQLSTATE `57014` alone.

## 4. Identify which cancellation control fired

Treat the origin as test input, not as something the database error code can reconstruct.

### Server timeout path

Record:

- the scope where `statement_timeout` was set;
- the normalized value returned by `SHOW statement_timeout`;
- SQLSTATE `57014`;
- whether the connection was reusable directly or after rollback;
- the larger outer-process deadline.

Prefer session-level `SET` for autocommit tests and `SET LOCAL` inside transaction tests. The PostgreSQL reference discourages setting `statement_timeout` globally in `postgresql.conf` because that affects every session. A focused integration test should not mutate a shared server-wide default.

### Driver cancel path

Record:

- the target backend PID internally, without publishing it as durable identity;
- evidence that the expected statement was active;
- the driver cancellation API used;
- SQLSTATE returned by the target operation;
- the same backend PID before and after cancellation;
- connection recovery.

A PostgreSQL `CancelRequest` opens a separate connection and includes secret key data associated with the original session. PostgreSQL aborts the current query only when that data matches a currently executing backend. The sender receives no direct confirmation, so the query's own result remains the proof.

### Administrative cancel path

If your product has an operator cancel feature, test it separately with `pg_cancel_backend(pid)`. PostgreSQL's [server-signaling reference](https://www.postgresql.org/docs/current/functions-admin.html#FUNCTIONS-ADMIN-SIGNAL) allows a role to cancel its own backend, a backend owned by a role it belongs to, or a backend when it has `pg_signal_backend`; only superusers can cancel superuser backends.

Do not grant `pg_signal_backend` merely to test ordinary application cancellation. The driver-cancel path uses the target connection's cancellation key and needs no cluster-wide signaling privilege. Keep administrative cancellation in a dedicated authorization test.

## 5. Diagnose a failed cancellation proof

### The query completed instead of timing out

Confirm `SHOW statement_timeout` on the same connection that runs the statement. Check for a later `SET`, pool checkout hook, role default, or transaction boundary that changed the setting. Use work that is unquestionably longer than the budget; `pg_sleep(10)` with a 200 ms server budget leaves a wide margin.

If a framework sends multiple statements in one simple-query message, remember that PostgreSQL 13 and newer apply `statement_timeout` to each statement separately. Do not assume one budget covers the entire SQL string on every supported major.

### `cancel_safe()` returned but the query succeeded

That outcome is allowed by the cancellation protocol. The query may have completed between the observer check and the cancel reaching the backend. Require the target future to return `57014`; do not treat the cancel method's return as proof.

For a fast query, use a deterministic server-side blocker or `pg_sleep` in a disposable database. Do not make a production query artificially expensive just to widen the cancellation window.

### The next statement returns `25P02`

The canceled statement ran inside an explicit transaction. Roll back before connection reuse. If a pool owns the connection, verify its release path performs that rollback or discards the connection.

The [Postgres MCP error-handling guide](/blog/postgres-mcp-server-error-handling-coding-agents/) follows the same rule: classify the stable database condition, preserve the failing layer, and choose remediation from the operation context.

PGSandbox's current [MCP error normalization](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/8abd97b9256de73665fbd9e9612f24d57a454677/rust-src/mcp.rs#L1596-L1600) maps SQLSTATE `57014` to the envelope code `statement_timeout`. PostgreSQL also uses `57014` for explicit client cancellation, so that envelope code cannot prove the origin by itself. Use a continuously owned driver connection and the controlled trigger path in this guide when provenance matters; separate `run_sql` calls do not preserve that live session.

### The test reports `55P03`, `40P01`, or `40001`

The harness exercised a different concurrency branch:

| SQLSTATE | Condition | Correct proof |
| --- | --- | --- |
| `55P03` | `lock_not_available` | Lock-acquisition budget and holder release |
| `40P01` | `deadlock_detected` | Two incompatible lock orders and one victim |
| `40001` | `serialization_failure` | Whole-transaction replay from a fresh snapshot |
| `57014` | `query_canceled` | Statement timeout or cancel provenance plus recovery |

Use the [serialization retry guide](/blog/test-postgres-serialization-failure-retries/) for `40001`. Do not collapse all four codes into one generic retry branch.

### Only the outer process timeout fires

The database cancellation path did not prove itself. Preserve the outer timeout as a failed harness result, close the child, and inspect the server setting, driver call, target activity observation, and network path. Increasing the outer deadline hides the issue unless the database budget was intentionally longer.

### The observer cannot see the target query text

Both harness connections should use the same sandbox role. PostgreSQL limits visibility of session details for ordinary roles, but a role can inspect its own sessions. If a proxy changes backend identity or the environment restricts `pg_stat_activity`, use a server-observable latch or blocker that proves the statement has started before cancellation. An application event emitted immediately before `execute()` is weaker evidence because the cancel can still arrive before PostgreSQL begins the statement.

## Common statement-timeout testing mistakes

### Matching the English error message

Messages can differ by cause, driver, server version, and localization. Assert the driver's SQLSTATE property equals `57014`. Record the activated control separately.

### Using only a wall-clock assertion

A slow CI runner can violate a narrow elapsed-time window even when cancellation is correct. Prove the configured budget, canceled SQLSTATE, incomplete long statement, and recovery. Keep elapsed duration as diagnostic data, not the only pass/fail condition.

### Setting a global timeout

A server-wide timeout can break migrations, maintenance, and unrelated tests. Scope the setting to the session, transaction, role, or database that owns the budget. This guide uses session and transaction scope because they are self-contained.

### Retrying a canceled write blindly

`57014` says the query was canceled; it does not prove that every surrounding side effect is safe to repeat. Define idempotency and the full operation boundary before adding automatic retries.

### Sharing the test database

A deliberate ten-second sleep and cancellation path should not consume a shared development connection or inherit unrelated role settings. Create a disposable database and scoped role for the task, then delete them when the proof ends.

### Printing connection details

Database URLs include credentials. Emit stable codes, control names, safe timing settings, booleans, and cleanup state. Never include the injected URL in CI logs or a PR description.

## PR-ready cancellation proof

Attach a compact, credential-free summary:

```text
Postgres cancellation proof
- PostgreSQL major: 18
- Server timeout: 200ms -> SQLSTATE 57014
- Autocommit recovery: SELECT 42 passed on the same connection
- Client cancel: target observed active -> SQLSTATE 57014
- Client cancel: backend PID unchanged after recovery
- Transaction recovery: 57014 -> 25P02 -> rollback -> SELECT 42 passed
- Transaction rollback: pre-cancel marker count is 0
- Outer process deadline: 30s, not reached
- Cleanup: succeeded
```

This is enough for a reviewer to distinguish the database condition, cancellation origin, transaction behavior, and sandbox lifecycle. Keep full structured child output in CI artifacts when needed, but do not paste credentials or unbounded logs into the PR.

## Frequently asked questions

### What SQLSTATE does Postgres use for a statement timeout?

PostgreSQL uses SQLSTATE `57014`, named `query_canceled`, when `statement_timeout` cancels a statement. The same code can also result from an explicit query-cancel request, so record which control the test activated instead of inferring the origin from SQLSTATE alone.

### What is the difference between `statement_timeout` and `lock_timeout`?

`statement_timeout` bounds the full server-side statement. `lock_timeout` applies only while PostgreSQL is waiting to acquire a lock. A lock timeout normally maps to `55P03`; a statement cancellation maps to `57014`.

### Can a Postgres connection be reused after query cancellation?

Yes, after the connection returns to a valid transaction state. In autocommit, the failed implicit transaction rolls back and the connection can usually run another statement immediately. Inside an explicit transaction, call `ROLLBACK` before reuse; statements issued first should fail with `25P02`.

### Does a successful client cancel call prove the query was canceled?

No. PostgreSQL sends no direct success response to the cancel-request connection, and the target query may finish before the signal takes effect. The original query must return a cancellation error such as SQLSTATE `57014`.

### Should applications retry SQLSTATE `57014` automatically?

Not by default. First identify whether a server timeout, client deadline, operator action, or another control initiated cancellation. Then decide whether the complete operation is safe to replay. Reads and writes often need different policies.

### Why run cancellation tests in a disposable Postgres database?

A disposable database isolates deliberately slow or canceled statements from shared application state and gives the test a scoped credential, outer deadline, TTL, and explicit cleanup result. PGSandbox supplies that lifecycle while the repository runs its real driver code.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Test Postgres statement timeouts and query cancellation",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Define the cancellation clocks", "text": "Set a PostgreSQL statement budget, an explicit client-cancel path, and a larger outer process deadline."},
        {"@type": "HowToStep", "position": 2, "name": "Force server cancellation", "text": "Run bounded long work with statement_timeout and require SQLSTATE 57014."},
        {"@type": "HowToStep", "position": 3, "name": "Force explicit client cancellation", "text": "Observe the target backend running before sending the driver's cancel request, then require the target query to return SQLSTATE 57014."},
        {"@type": "HowToStep", "position": 4, "name": "Prove transaction recovery", "text": "Verify immediate autocommit reuse and require rollback after cancellation inside an explicit transaction."},
        {"@type": "HowToStep", "position": 5, "name": "Run inside disposable Postgres", "text": "Use pgsandbox with-database with a scoped role, outer timeout, credential-safe output, TTL, and explicit cleanup."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "What SQLSTATE does Postgres use for a statement timeout?", "acceptedAnswer": {"@type": "Answer", "text": "PostgreSQL uses SQLSTATE 57014, query_canceled. Explicit query cancellation can return the same code, so the harness must also record which control it activated."}},
        {"@type": "Question", "name": "What is the difference between statement_timeout and lock_timeout?", "acceptedAnswer": {"@type": "Answer", "text": "statement_timeout bounds the full server-side statement, while lock_timeout applies only during lock acquisition. They represent different failure and recovery branches."}},
        {"@type": "Question", "name": "Can a Postgres connection be reused after query cancellation?", "acceptedAnswer": {"@type": "Answer", "text": "Yes after transaction recovery. Autocommit normally permits immediate reuse; an explicit failed transaction requires rollback first."}},
        {"@type": "Question", "name": "Does a successful client cancel call prove the query was canceled?", "acceptedAnswer": {"@type": "Answer", "text": "No. The original target query must return a cancellation result because the cancel-request connection receives no direct confirmation that the server applied it."}},
        {"@type": "Question", "name": "Should applications retry SQLSTATE 57014 automatically?", "acceptedAnswer": {"@type": "Answer", "text": "No. Identify the cancellation origin and prove the complete operation is safe to replay before adding retries."}},
        {"@type": "Question", "name": "Why run cancellation tests in a disposable Postgres database?", "acceptedAnswer": {"@type": "Answer", "text": "A disposable database isolates deliberate slow-query work and supplies a scoped credential, process deadline, TTL, and explicit cleanup result."}}
      ]
    }
  ]
}
</script>
