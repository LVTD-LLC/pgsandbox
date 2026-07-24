---
title: "How to Test Postgres Connection Pool Failures Safely"
excerpt: "Reproduce pool acquisition timeouts and broken idle connections against disposable Postgres, then prove bounded failure, recovery, and cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-24"
updatedAt: "2026-07-24T06:00:00Z"
tags: ["Postgres", "connection pools", "integration testing", "Node.js", "coding agents"]
category: "Engineering"
metaTitle: "Test Postgres Connection Pool Failures Safely"
metaDescription: "Test node-postgres pool exhaustion and broken connections with bounded acquisition timeouts, recovery assertions, and disposable database cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-connection-pool-failures/"
heroImageUrl: ""
featured: false
sortOrder: 146
---
Postgres connection pool testing should verify two different failures: a request waiting too long for an application-pool client, and an established client becoming unusable after its PostgreSQL backend disappears. This guide uses node-postgres with a deliberately tiny pool, bounded acquisition timeouts, observable pool counters, and a disposable database. Then it verifies that the pool serves a new query after each failure.

Do not test this by opening connections until a shared PostgreSQL server refuses them. PostgreSQL connection-slot exhaustion is a cluster-wide condition. A safe repository test can saturate its own two-client pool without consuming every server slot, terminate only a backend owned by its sandbox role, and leave cleanup to the sandbox lifecycle.

This guide packages that evidence into a **Pool Failure Proof Contract**: capacity, pressure, classification, recovery, and cleanup. The result tells a coding agent and reviewer which layer failed and whether the application recovered without exposing a database URL.

## In this guide

- [Separate pool exhaustion from database exhaustion](#pool-exhaustion-vs-postgres-connection-exhaustion)
- [Use the Pool Failure Proof Contract](#the-pool-failure-proof-contract)
- [Create a deterministic Node.js harness](#1-create-a-deterministic-pool-failure-harness)
- [Test bounded pool acquisition](#2-prove-client-pool-saturation)
- [Test a broken idle connection](#3-prove-recovery-after-a-backend-disconnect)
- [Run the proof with PGSandbox](#4-run-the-proof-in-a-disposable-database)
- [Diagnose a failed proof](#5-diagnose-the-layer-that-failed)
- [Record review-ready evidence](#pr-ready-connection-pool-proof)

## Pool exhaustion vs Postgres connection exhaustion

An application pool and PostgreSQL enforce different limits.

A pool's `max` setting limits how many clients one process can hold. When every client is checked out, another request waits in that pool's queue. The node-postgres [Pool API](https://node-postgres.com/apis/pool) documents `connectionTimeoutMillis` as the acquisition bound and exposes `totalCount`, `idleCount`, and `waitingCount` for observing that state.

PostgreSQL's `max_connections` limits concurrent server connections. The current [connection settings](https://www.postgresql.org/docs/current/runtime-config-connection.html) also reserve slots for superusers and, when configured, roles with `pg_use_reserved_connections`. A server-side refusal can therefore occur before an ordinary application role reaches the headline maximum.

Keep the two conditions separate:

| Failure layer | Test condition | Evidence | Safe repository action |
| --- | --- | --- | --- |
| Application pool | All pool clients are checked out | `totalCount=max`, `idleCount=0`, `waitingCount=1`, bounded acquire error | Force with a tiny pool |
| PostgreSQL server | Ordinary connection slots are unavailable | Server connection error plus cluster connection counts | Observe in a dedicated server profile; do not force on shared Postgres |
| Network or backend | An established socket becomes unusable | Pool `error` event or query error, dead client removed | Terminate one same-role sandbox backend |
| Query execution | A checked-out client waits on SQL work | SQLSTATE such as `55P03` or `57014` | Test separately from acquisition |

The distinction changes the fix. A pool acquisition timeout may indicate leaked clients, slow queries, or request concurrency above the process budget. A PostgreSQL refusal points to the aggregate connection budget across every service and pool. A statement timeout or lock timeout means a client was acquired and SQL execution started.

The [Postgres deadlock and lock-timeout guide](/blog/test-postgres-deadlocks-lock-timeouts/) tests that last category. This guide stops at the connection boundary.

## The Pool Failure Proof Contract

A review-ready pool test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Capacity | What limit did this process own? | Driver, pool `max`, acquire timeout, and application name |
| Pressure | How was the limit reached? | Named checked-out clients and `waitingCount` before timeout |
| Classification | Which layer rejected or lost the request? | Pool acquisition, PostgreSQL connection, backend disconnect, or query execution |
| Recovery | Can the pool serve a fresh query afterward? | Replacement query plus final pool counters |
| Cleanup | What connections and database remain? | `pool.end()` and PGSandbox cleanup result |

This contract adds evidence that a generic "too many connections" checklist misses. It turns a symptom into a controlled integration test with a stable boundary: the application owns pooling; PostgreSQL owns sessions and server limits; PGSandbox owns the disposable database and scoped credentials.

## 1. Create a deterministic pool-failure harness

The harness below uses Node.js and node-postgres. It runs two proofs against one disposable database:

1. Hold both clients in a `max: 2` pool and require a third acquisition to fail within a short budget.
2. Return an identified client to the idle pool, terminate that same-role PostgreSQL backend from a separate connection, observe removal, and prove a replacement query succeeds.

Save it as `tests/postgres_pool_failure_proof.mjs`:

```js
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import pg from "pg";

const { Client, Pool } = pg;
const connectionString =
  process.env.PGSANDBOX_DATABASE_URL ?? process.env.DATABASE_URL;

assert.ok(connectionString, "PGSANDBOX_DATABASE_URL or DATABASE_URL is required");

function snapshot(pool) {
  return {
    total: pool.totalCount,
    idle: pool.idleCount,
    waiting: pool.waitingCount,
  };
}

async function proveAcquireTimeout() {
  const pool = new Pool({
    connectionString,
    application_name: "pool-proof-saturation",
    max: 2,
    connectionTimeoutMillis: 250,
    idleTimeoutMillis: 1_000,
  });

  const held = [];
  try {
    held.push(await pool.connect());
    held.push(await pool.connect());
    assert.deepEqual(snapshot(pool), { total: 2, idle: 0, waiting: 0 });

    const startedAt = Date.now();
    const pending = pool.connect();
    await delay(25);
    assert.equal(pool.waitingCount, 1);
    const heldClientHealth = await held[0].query("SELECT 1 AS ok");
    assert.equal(heldClientHealth.rows[0].ok, 1);

    const acquisitionError = await pending.then(
      () => {
        throw new Error("queued acquisition unexpectedly succeeded");
      },
      (error) => error
    );
    assert.ok(acquisitionError instanceof Error);
    const elapsedMs = Date.now() - startedAt;
    assert.ok(elapsedMs >= 200 && elapsedMs < 5_000, { elapsedMs });

    held.pop().release();
    const recovery = await pool.query("SELECT 1 AS ok");
    assert.equal(recovery.rows[0].ok, 1);

    return {
      capacity: 2,
      acquireTimeoutMs: 250,
      elapsedMs,
      errorName: acquisitionError.name,
      errorCode: acquisitionError.code ?? null,
      recovered: true,
      final: snapshot(pool),
    };
  } finally {
    for (const client of held) client.release();
    await pool.end();
  }
}

async function proveIdleBackendRecovery() {
  const pool = new Pool({
    connectionString,
    application_name: "pool-proof-disconnect",
    max: 2,
    connectionTimeoutMillis: 500,
    idleTimeoutMillis: 5_000,
  });

  let resolvePoolError;
  const poolError = new Promise((resolve) => {
    resolvePoolError = resolve;
  });
  let resolvePoolRemoval;
  const poolRemoval = new Promise((resolve) => {
    resolvePoolRemoval = resolve;
  });
  pool.on("error", (error, client) => {
    resolvePoolError({ code: error.code ?? null, client });
  });
  pool.on("remove", resolvePoolRemoval);

  try {
    const victim = await pool.connect();
    let victimPid;
    try {
      const victimResult = await victim.query(
        "SELECT pg_backend_pid() AS pid"
      );
      victimPid = victimResult.rows[0].pid;
    } finally {
      victim.release();
    }

    const killer = new Client({
      connectionString,
      application_name: "pool-proof-terminator",
    });
    await killer.connect();
    try {
      const terminated = await killer.query(
        "SELECT pg_terminate_backend($1) AS terminated",
        [victimPid]
      );
      assert.equal(terminated.rows[0].terminated, true);
    } finally {
      await killer.end();
    }

    const observed = await Promise.race([
      poolError,
      delay(2_500).then(() => {
        throw new Error("pool did not report the terminated idle client");
      }),
    ]);
    const removedClient = await Promise.race([
      poolRemoval,
      delay(2_500).then(() => {
        throw new Error("pool did not remove the terminated idle client");
      }),
    ]);
    assert.equal(removedClient, observed.client);
    assert.deepEqual(snapshot(pool), { total: 0, idle: 0, waiting: 0 });

    const recovery = await pool.query(
      "SELECT current_database() AS database, 1 AS ok"
    );
    assert.equal(recovery.rows[0].ok, 1);

    return {
      idleClientErrorCode: observed.code,
      removed: true,
      recovered: true,
      final: snapshot(pool),
    };
  } finally {
    await pool.end();
  }
}

const proof = {
  acquireTimeout: await proveAcquireTimeout(),
  idleBackendRecovery: await proveIdleBackendRecovery(),
};

console.log(JSON.stringify(proof));
```

The script prints only the proof. It never prints `connectionString`, `DATABASE_URL`, `PGPASSWORD`, or the database user.

## 2. Prove client-pool saturation

The first proof checks out exactly two clients and keeps them checked out. Because `max` is two, the third `pool.connect()` cannot create another client. It enters the acquisition queue, which makes `waitingCount` equal one.

The important assertion is not the English error message. node-postgres produces a client-side acquisition error here, so there may be no PostgreSQL SQLSTATE. Classify it from the controlled preconditions:

- the pool had reached its configured capacity;
- both clients were checked out;
- one acquisition was waiting;
- PostgreSQL still accepted queries on the held clients;
- the acquisition failed near the configured 250 ms budget.

The node-postgres [pooling guide](https://node-postgres.com/features/pooling) warns that every checked-out client must be returned. A missing `release()` can leave the pool empty forever from the caller's perspective. The harness uses `finally` and then calls `pool.end()` so the failure path itself does not become a leak.

After the expected timeout, the test releases one client and runs `SELECT 1`. That recovery query proves the pool was saturated rather than permanently broken.

### Test leak handling in application code, not by leaking the harness

If the real code uses `pool.connect()`, add unit tests around every error path and require release in `finally`:

```js
const client = await pool.connect();
try {
  await client.query("BEGIN");
  await runWork(client);
  await client.query("COMMIT");
} catch (error) {
  await client.query("ROLLBACK");
  throw error;
} finally {
  client.release();
}
```

For one independent statement, prefer `pool.query()`. The pool checks out and returns the client internally, reducing the leak surface.

## 3. Prove recovery after a backend disconnect

The second proof targets a different failure: an idle pooled connection whose PostgreSQL backend is terminated.

PostgreSQL's [`pg_stat_activity` documentation](https://www.postgresql.org/docs/current/monitoring-stats.html) describes one row per server process, including `pid`, `application_name`, and state. The harness gets its own backend PID directly with `pg_backend_pid()` and terminates only that PID.

The current [`pg_terminate_backend` reference](https://www.postgresql.org/docs/current/functions-admin.html) allows termination when the caller is a member of the target backend's role; only superusers can terminate superuser backends. Both connections in this test use the same scoped sandbox role. The proof does not need `pg_signal_backend`, a superuser credential, or access to another application's sessions.

node-postgres documents that an idle client can emit an error after a backend or network failure. The pool removes that client. The test waits for the pool-level `error` event, then runs a new query. A passing recovery assertion shows that one broken idle socket did not leave the process unable to use PostgreSQL.

This is narrower than a full outage test. It does not prove DNS failure, TLS rotation, proxy behavior, server restart, or failover. Those require an environment whose network and server lifecycle the test exclusively owns. Do not simulate them against a shared local profile by stopping PostgreSQL.

## 4. Run the proof in a disposable database

Use the repository's existing node-postgres dependency, or add it to the repository once with `npm install --save-dev pg`. Then run the harness as the child of `pgsandbox with-database`:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 15 \
  --cleanup always \
  --timeout-seconds 30 \
  --result-format json \
  -- node tests/postgres_pool_failure_proof.mjs
```

Install and configure PGSandbox first with the [setup guide](/docs/install/). The example selects PostgreSQL 18; use a configured `--profile` or another installed major when that better matches the repository under test.

PGSandbox creates the database and restricted role, injects `DATABASE_URL`, `PGSANDBOX_DATABASE_URL`, and standard libpq variables, supervises the child process, redacts its captured output, and applies the selected cleanup policy. The [agent test-session docs](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/e1810ef5777491d340a0166dd0ef67c15246b206/docs/agent-testing.md) define the result fields and exit behavior.

Use `--cleanup always` in CI and unattended agent work. Use `--cleanup on-success` only when a human will inspect a failed database before its TTL expires. The [sandbox TTL guide](/blog/postgres-sandbox-ttl-values/) explains how to bound that recovery window.

`with-database` is the product-led boundary that makes this useful:

- the repository runs its real pool implementation;
- the child receives a task-scoped role rather than lifecycle authority;
- backend termination stays inside that role;
- an outer timeout stops a hung proof;
- cleanup remains explicit and machine-readable.

The broader [disposable Postgres integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) covers migrations, test commands, environment aliases, and session result handling.

## 5. Diagnose the layer that failed

When the harness fails, read the evidence in order.

### The third acquisition did not wait

Confirm `max: 2` was applied to the pool being tested. Multiple `Pool` instances create separate budgets. An application that creates one pool per request or module can exceed its intended aggregate connection count even when each pool is small.

The node-postgres [pool-sizing guide](https://node-postgres.com/guides/pool-sizing) recommends budgeting across every application instance and leaving server headroom. PostgreSQL connection capacity is shared; one process's `max` is not a cluster reservation.

### The acquisition never timed out

Set a finite acquisition timeout in the test. A zero `connectionTimeoutMillis` means no timeout. Also put a larger outer timeout around the whole child process so a pool regression cannot hang CI.

### PostgreSQL refused the first or second client

This is not application-pool saturation. Check the server's connection budget and other sessions. On a profile where the sandbox role can see only limited activity details, use safe aggregate evidence and ask the profile operator to inspect cluster-wide state.

Do not "fix" the test by increasing `max_connections` automatically. PostgreSQL allocates resources based on that value, and it is a server-start setting. Pool size, service instance count, query duration, administrative headroom, and any external pooler all belong in the capacity decision.

### `pg_terminate_backend` was denied

Verify the terminator and target use the same sandbox role and that the PID came from the victim connection. Do not grant broad signaling privileges just to make the test pass. If a proxy hides backend identity or blocks the operation, keep the acquisition proof and run disconnect testing in a dedicated environment that owns the proxy lifecycle.

### The pool reported the error but did not recover

Check that the error listener is installed before terminating the idle client. Then inspect whether the driver removed the dead client and whether a new connection attempt reached PostgreSQL. The [MCP error-handling guide](/blog/postgres-mcp-server-error-handling-coding-agents/) shows the same classification discipline for PGSandbox tool failures: preserve the failure layer and stable code rather than retrying every database-looking error.

## Common Postgres connection pool testing mistakes

### Exhausting a shared server

Opening clients until PostgreSQL refuses ordinary roles can disrupt unrelated work. Saturate a small application pool instead. Force server exhaustion only on a dedicated profile with an operator-approved connection budget.

### Matching only error text

Pool acquisition errors may not have SQLSTATE because PostgreSQL never saw a query. Record the pool state, configured timeout, elapsed time, and driver error class or code when available.

### Testing with a mock database

A mock can test application branching. It cannot prove driver pool counters, PostgreSQL backend termination, socket removal, or replacement-connection behavior. Keep fast unit tests, then add one bounded real-Postgres proof.

### Forgetting aggregate pool size

`max: 10` across eight service processes can create up to 80 application connections before accounting for workers, migrations, monitoring, or administrative access. Test the per-process contract and review the deployment-wide budget separately.

### Printing the injected connection URL

Connection URLs contain credentials. Record the safe sandbox ID from the PGSandbox session result, not the URL or password.

## PR-ready connection pool proof

Record a compact result with a pool-related patch:

```text
Postgres pool failure proof
- Target: PostgreSQL=<major>, sandbox=<safe database ID>
- Driver: node-postgres, pool max=2, acquire timeout=250ms
- Pressure: total=2, idle=0, waiting=1 before timeout
- Classification: client-pool acquisition timeout; PostgreSQL remained queryable
- Disconnect: one same-role idle backend terminated and removed
- Recovery: SELECT 1 passed after saturation and disconnect
- Session: status=<status>, child exit=<code>, elapsed=<duration>
- Cleanup: policy=always, deleted=<yes/no>, error=<stable code or none>
```

That proof is small enough for a reviewer to verify. It names the pool boundary, failure mechanism, recovery, and database lifecycle without pasting credentials or unbounded logs.

## Frequently asked questions

### How do you test Postgres connection pool exhaustion?

Configure a small application pool, check out every client, start one more acquisition, and assert it waits and fails within a finite acquisition timeout. Record pool capacity and counters, then release a client and prove a fresh query succeeds. Do not exhaust a shared PostgreSQL server.

### What is the difference between pool exhaustion and max_connections?

Pool exhaustion happens inside an application or proxy when all pooled clients are busy. PostgreSQL `max_connections` is a server-wide limit shared across clients, services, and administrative sessions. The first may occur while PostgreSQL has free slots; the second can reject a new connection before it enters an application pool.

### Should a connection-pool timeout have a PostgreSQL SQLSTATE?

Not necessarily. If a request times out waiting for a pooled client, PostgreSQL may never receive a connection or query, so there is no server SQLSTATE. Classify the failure using driver metadata, pool counters, the acquisition budget, and elapsed time.

### How can you test recovery from a broken pooled connection?

Identify one idle connection's backend PID, terminate only that same-role PostgreSQL backend, observe the pool's idle-client error path, and run a new query. The test passes when the dead client is removed and a replacement connection serves the query.

### Can PGSandbox test PgBouncer or managed-database failover?

PGSandbox can provide the disposable database and scoped credential used by the application test. It does not own an external pooler's network or a managed provider's failover lifecycle. Test those behaviors in a dedicated environment that owns those components.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Test Postgres connection pool failures safely",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Create a tiny pool", "text": "Configure the real application driver with a pool maximum of two and a finite acquisition timeout."},
        {"@type": "HowToStep", "position": 2, "name": "Saturate the pool", "text": "Check out both clients, start a third acquisition, and record total, idle, and waiting counters."},
        {"@type": "HowToStep", "position": 3, "name": "Assert bounded failure", "text": "Require the queued acquisition to fail near its configured budget without matching only English error text."},
        {"@type": "HowToStep", "position": 4, "name": "Test backend recovery", "text": "Terminate one same-role idle backend, observe its removal from the pool, and prove a replacement query succeeds."},
        {"@type": "HowToStep", "position": 5, "name": "Clean up the database", "text": "Run the proof through pgsandbox with-database with an outer timeout, credential-safe result, and explicit cleanup policy."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do you test Postgres connection pool exhaustion?", "acceptedAnswer": {"@type": "Answer", "text": "Use a small application pool, check out every client, require one more acquisition to fail within a finite timeout, then release a client and prove a fresh query succeeds."}},
        {"@type": "Question", "name": "What is the difference between pool exhaustion and max_connections?", "acceptedAnswer": {"@type": "Answer", "text": "Pool exhaustion is a client or proxy limit. PostgreSQL max_connections is a server-wide connection limit shared across services and administrative sessions."}},
        {"@type": "Question", "name": "Should a connection-pool timeout have a PostgreSQL SQLSTATE?", "acceptedAnswer": {"@type": "Answer", "text": "Not necessarily. If the request times out before it gets a pooled client, PostgreSQL never receives the query and cannot return a SQLSTATE."}},
        {"@type": "Question", "name": "How can you test recovery from a broken pooled connection?", "acceptedAnswer": {"@type": "Answer", "text": "Terminate one same-role idle PostgreSQL backend, observe the pool error path, and run a new query to prove that the pool creates or uses a healthy replacement connection."}},
        {"@type": "Question", "name": "Can PGSandbox test PgBouncer or managed-database failover?", "acceptedAnswer": {"@type": "Answer", "text": "PGSandbox supplies the disposable database and scoped credential, but external pooler and managed failover behavior require a dedicated environment that owns those components."}}
      ]
    }
  ]
}
</script>
