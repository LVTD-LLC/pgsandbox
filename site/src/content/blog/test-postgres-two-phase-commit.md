---
title: "How to Test PostgreSQL Two-Phase Commit"
excerpt: "Prove prepare-state invisibility, pg_prepared_xacts identity, cross-session commit and rollback, and zero remaining prepared transactions."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-02"
updatedAt: "2026-08-02T06:00:00Z"
tags: ["Postgres", "two-phase commit", "prepared transactions", "integration testing", "coding agents"]
category: "Engineering"
metaTitle: "Test PostgreSQL Two-Phase Commit"
metaDescription: "Test PostgreSQL two-phase commit with prepared-state visibility, cross-session commit and rollback, recovery checks, and safe cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-two-phase-commit/"
heroImageUrl: ""
featured: false
sortOrder: 153
---
Test PostgreSQL two-phase commit by preparing a transaction under a unique global transaction identifier, inspecting it from a second session, and making both possible recovery decisions. A complete test proves that uncommitted rows stay invisible, the prepared transaction survives its original connection, `COMMIT PREPARED` makes one change visible, `ROLLBACK PREPARED` discards another, and no prepared transaction remains afterward.

A happy-path `PREPARE TRANSACTION` demo misses the operational risk. If a test fails after preparation, the leftover transaction can retain locks and block database disposal. Recovery and cleanup belong in the test contract.

Use five checks for a reviewable **Prepared Transaction Proof Contract**: configuration, preparation, identity, decision, and hygiene. PGSandbox MCP provides the disposable database and scoped role. Your PostgreSQL operator must explicitly enable prepared transactions on the selected profile; PGSandbox does not change cluster startup settings.

*Published and last updated August 2, 2026.*

The complete proof has five steps:

1. Require a profile where `max_prepared_transactions` is greater than zero.
2. Prepare a write under a unique GID and close the originating connection.
3. Prove the write is invisible while `pg_prepared_xacts` records the correct owner and database.
4. Commit one prepared transaction and roll back a second from another session.
5. Assert both GIDs are gone before PGSandbox removes the disposable database.

## In this guide

- [Understand PostgreSQL two-phase commit](#what-postgresql-two-phase-commit-does)
- [Use the Prepared Transaction Proof Contract](#the-prepared-transaction-proof-contract)
- [Configure a test profile](#1-use-a-profile-that-enables-prepared-transactions)
- [Create the deterministic harness](#2-create-a-deterministic-two-phase-commit-test)
- [Run it with PGSandbox](#3-run-the-proof-in-a-disposable-postgres-database)
- [Interpret the evidence](#4-interpret-the-two-phase-commit-proof)
- [Plan failure recovery](#5-treat-recovery-as-part-of-the-test)
- [Record PR-ready evidence](#pr-ready-two-phase-commit-proof)

## What PostgreSQL two-phase commit does

PostgreSQL two-phase commit separates a transaction into a prepare phase and a later commit-or-rollback decision. `PREPARE TRANSACTION` completes the first phase and detaches the transaction from the current session. An external transaction manager later issues `COMMIT PREPARED` or `ROLLBACK PREPARED` after it knows whether the wider operation can commit everywhere.

The current PostgreSQL [two-phase transaction documentation](https://www.postgresql.org/docs/current/two-phase.html) names those three commands and describes the feature as a protocol for coordinating multiple transactional systems. A single-database harness therefore tests one PostgreSQL participant's state machine. It does not prove that a distributed coordinator handles every participant or every crash window correctly.

| Phase | PostgreSQL command | Observable result |
| --- | --- | --- |
| Work | ordinary SQL inside a transaction | Changes are visible only inside that transaction |
| Prepare | `PREPARE TRANSACTION 'gid'` | Work detaches from the session and appears in `pg_prepared_xacts` |
| Commit decision | `COMMIT PREPARED 'gid'` | Prepared changes become visible and the catalog row disappears |
| Rollback decision | `ROLLBACK PREPARED 'gid'` | Prepared changes are discarded and the catalog row disappears |

### Prepared transactions are not prepared statements

A prepared transaction is durable in-doubt work waiting for a commit or rollback decision. A prepared statement is a parsed SQL statement that a session may execute later with parameter values. Similar names do not imply similar lifecycle or risk.

Use `pg_prepared_xacts` to inspect prepared transactions. Do not use `pg_prepared_statements`, which reports session-local prepared statements.

### The feature is disabled by default

PostgreSQL's [`max_prepared_transactions` reference](https://www.postgresql.org/docs/current/runtime-config-resource.html#GUC-MAX-PREPARED-TRANSACTIONS) says the default value is `0`, which disables prepared transactions. The setting can only be changed at server start. This is a cluster capability, not a database-level toggle that a sandbox role should change.

That default is deliberate. PostgreSQL's [`PREPARE TRANSACTION` reference](https://www.postgresql.org/docs/current/sql-prepare-transaction.html) recommends leaving the feature disabled when no external transaction manager owns recovery. Prepared transactions retain their locks and can interfere with `VACUUM` if they remain open too long.

For PGSandbox, the practical rule is simple: target a dedicated development profile whose PostgreSQL cluster was started with a nonzero value. Do not enable 2PC on a shared server merely to make one test pass.

## The Prepared Transaction Proof Contract

A reviewable two-phase commit test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Configuration | Can this cluster hold prepared transactions? | `SHOW max_prepared_transactions` returns a positive integer |
| Preparation | Did phase one detach durable, still-invisible work? | Original connection closes; another session cannot see the row |
| Identity | Can recovery select the exact transaction safely? | `pg_prepared_xacts` matches GID, owner, and current database |
| Decision | Do both terminal paths work from another session? | Commit exposes one row; rollback keeps the other absent |
| Hygiene | Did the test resolve every GID it created? | No matching `pg_prepared_xacts` rows before database cleanup |

Identity and hygiene are what most 2PC snippets omit. A bare prepare-and-commit test does not show whether recovery can find the correct participant after the original process disappears. It also does not protect the next test run from an orphaned transaction.

PostgreSQL requires a GID to be unique among currently prepared transactions and shorter than 200 bytes. The harness below uses a readable test prefix plus a UUID, then filters every catalog query by both GID and current database. It never rolls back a prepared transaction it did not create.

## 1. Use a profile that enables prepared transactions

Check the target server before running destructive setup:

```sql
SHOW max_prepared_transactions;
```

If the result is `0`, stop. An operator must configure the PostgreSQL cluster and restart it. For example, a dedicated test cluster may contain this setting in `postgresql.conf`:

```conf
max_prepared_transactions = 10
```

The right value depends on the coordinator's maximum in-flight work. PostgreSQL suggests a value at least as large as `max_connections` when every session may have a prepared transaction pending. A narrow integration-test cluster can use a smaller, deliberate cap when its concurrency is bounded.

Register that server as an explicit PGSandbox profile, then select the profile by name. PGSandbox remains responsible for creating and tracking the task database. The profile operator remains responsible for PostgreSQL startup configuration.

This boundary follows the [PGSandbox architecture](/docs/architecture/): lifecycle commands use the configured admin connection, while test SQL runs through a scoped role inside the sandbox database. PGSandbox does not install, host, or reconfigure your PostgreSQL server.

## 2. Create a deterministic two-phase commit test

The following Psycopg 3 harness prepares two inserts. It commits the first GID, rolls back the second, and inspects each state from an autocommit recovery connection. Autocommit matters because PostgreSQL requires `COMMIT PREPARED` and `ROLLBACK PREPARED` to run outside a transaction block.

Save it as `tests/postgres_two_phase_commit_proof.py`:

```python
import json
import os
import uuid

import psycopg
from psycopg import sql


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]
RUN_ID = uuid.uuid4().hex
GIDS = [
    f"pgsandbox-2pc-{RUN_ID}-commit",
    f"pgsandbox-2pc-{RUN_ID}-rollback",
]


def connect(*, autocommit=False):
    return psycopg.connect(
        DATABASE_URL,
        autocommit=autocommit,
        connect_timeout=5,
    )


def prepared_command(command, gid):
    return sql.SQL("{} PREPARED {}").format(
        sql.SQL(command),
        sql.Literal(gid),
    )


def prepare_insert(gid, item_id, note):
    conn = connect()
    try:
        conn.execute(
            "INSERT INTO two_phase_items (id, note) VALUES (%s, %s)",
            (item_id, note),
        )
        conn.execute(
            sql.SQL("PREPARE TRANSACTION {}").format(sql.Literal(gid))
        )
    finally:
        conn.close()


def prepared_record(conn, gid):
    return conn.execute(
        """
        SELECT gid, owner::text, database::text
        FROM pg_prepared_xacts
        WHERE gid = %s
          AND database = current_database()
        """,
        (gid,),
    ).fetchone()


def visible_ids(conn):
    return [
        row[0]
        for row in conn.execute(
            "SELECT id FROM two_phase_items ORDER BY id"
        ).fetchall()
    ]


def resolve_if_present(conn, gid):
    if prepared_record(conn, gid):
        conn.execute(prepared_command("ROLLBACK", gid))


def main():
    evidence = {}
    with connect(autocommit=True) as recovery:
        enabled = int(
            recovery.execute("SHOW max_prepared_transactions").fetchone()[0]
        )
        if enabled <= 0:
            raise RuntimeError(
                "prepared transactions are disabled: select a test profile "
                "with max_prepared_transactions > 0"
            )

        recovery.execute("DROP TABLE IF EXISTS two_phase_items")
        recovery.execute(
            """
            CREATE TABLE two_phase_items (
                id integer PRIMARY KEY,
                note text NOT NULL
            )
            """
        )

        try:
            prepare_insert(GIDS[0], 1, "commit branch")
            commit_record = prepared_record(recovery, GIDS[0])
            assert commit_record is not None
            assert commit_record[1] == recovery.info.user
            assert commit_record[2] == recovery.info.dbname
            assert visible_ids(recovery) == []

            recovery.execute(prepared_command("COMMIT", GIDS[0]))
            assert prepared_record(recovery, GIDS[0]) is None
            assert visible_ids(recovery) == [1]

            prepare_insert(GIDS[1], 2, "rollback branch")
            rollback_record = prepared_record(recovery, GIDS[1])
            assert rollback_record is not None
            assert visible_ids(recovery) == [1]

            recovery.execute(prepared_command("ROLLBACK", GIDS[1]))
            assert prepared_record(recovery, GIDS[1]) is None
            assert visible_ids(recovery) == [1]

            evidence = {
                "max_prepared_transactions": enabled,
                "commit_prepared_record": commit_record,
                "rollback_prepared_record": rollback_record,
                "visible_ids_after_decisions": visible_ids(recovery),
                "remaining_test_gids": [],
            }
        finally:
            for gid in GIDS:
                resolve_if_present(recovery, gid)
            remaining = [gid for gid in GIDS if prepared_record(recovery, gid)]
            if remaining:
                raise AssertionError(f"prepared transactions remain: {remaining}")

        print(json.dumps(evidence, sort_keys=True))


if __name__ == "__main__":
    main()
```

`sql.Literal` quotes each GID as a value. Do not concatenate an arbitrary coordinator ID into `PREPARE`, `COMMIT PREPARED`, or `ROLLBACK PREPARED` SQL.

The code deliberately uses ordinary SQL so the server-side transitions stay visible. Psycopg also provides [`tpc_begin()`, `tpc_prepare()`, `tpc_commit()`, `tpc_rollback()`, and `tpc_recover()`](https://www.psycopg.org/psycopg3/docs/basic/transactions.html#two-phase-commit-protocol-support). Use those APIs when they match the transaction manager in production; keep the same external assertions around them.

## 3. Run the proof in a disposable Postgres database

Install the test dependency in your project environment:

```bash
python -m pip install "psycopg[binary]>=3.2,<4"
```

Run the harness against the prepared-transaction-enabled profile:

```bash
pgsandbox with-database \
  --profile prepared-tx-test \
  --name-hint postgres-2pc-proof \
  --ttl-minutes 30 \
  --cleanup on-success \
  -- python tests/postgres_two_phase_commit_proof.py
```

`with-database` creates a tracked database and scoped login role, injects its connection string into the child process, and captures bounded output. The [disposable Postgres integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) covers the broader one-shot session contract.

Use `on-success` rather than assuming that cleanup can always delete a failed 2PC test immediately. If the child process is killed after `PREPARE TRANSACTION` but before its `finally` block, PostgreSQL may still have a prepared transaction attached to the sandbox database. Preserve the failed sandbox, inspect it, resolve the known GID, and then delete it.

The harness uses the same scoped sandbox role to prepare and recover. PostgreSQL's [`COMMIT PREPARED` documentation](https://www.postgresql.org/docs/current/sql-commit-prepared.html) and [`ROLLBACK PREPARED` documentation](https://www.postgresql.org/docs/current/sql-rollback-prepared.html) require the original user or a superuser for the terminal decision, though the recovery session can differ from the session that prepared the work.

## 4. Interpret the two-phase commit proof

Each assertion covers a different failure mode.

### Configuration: fail before creating in-doubt work

The first query turns a hidden infrastructure assumption into explicit evidence:

```json
{"max_prepared_transactions": 10}
```

A test that skips this check usually fails later with a less useful server error. Worse, a team may enable prepared transactions casually and never assign ownership for recovery. The configuration assertion should link to the exact test profile and its operator.

### Preparation: prove invisibility after the original session ends

After `PREPARE TRANSACTION`, the harness closes the writer connection. The recovery connection must still find the GID in `pg_prepared_xacts`, but `visible_ids()` must return an empty list.

PostgreSQL documents that prepared work is no longer associated with the original session and can be completed from another session. It also states that the effects are not visible again unless the transaction is committed. Closing the phase-one process simulates the crash boundary while proving that another process can apply the recorded decision.

### Identity: match GID, owner, and database

The [`pg_prepared_xacts` view](https://www.postgresql.org/docs/current/view-pg-prepared-xacts.html) contains one row per prepared transaction and exposes the GID, preparation time, owner, and database. The test checks three identity fields:

- the exact random GID created by this run;
- the scoped sandbox role that prepared it;
- the disposable database selected for the test.

Checking only a GID prefix is unsafe on a shared cluster. A recovery worker needs a durable mapping from the global operation to every participant's exact GID, database, and intended decision.

### Decision: prove both terminal paths

The commit branch ends with visible row `1`. The rollback branch prepares row `2`, confirms it is still invisible, and discards it. The final visible state is exactly `[1]`.

This paired assertion distinguishes four bugs that one happy path misses: a coordinator that always commits, a rollback command sent inside a transaction, recovery under the wrong role, and a visibility check performed before the terminal command completes.

### Hygiene: require a zero-GID exit state

The test's `finally` block rolls back only GIDs from its own in-memory list. It then verifies that neither remains. It never performs a blanket cleanup of every prepared transaction on the server.

That narrow cleanup follows the same ownership principle as the [per-sandbox Postgres role guide](/blog/per-sandbox-postgres-roles-coding-agents/): task code should resolve only resources it can identify as its own. Cluster-wide recovery belongs to an operator or transaction manager with durable decision records.

## 5. Treat recovery as part of the test

Two-phase commit is safe only when the system can finish a decision after the original worker disappears. Your production test plan should include more than the single-participant harness above.

### Test the coordinator's durable decision log

Before sending `COMMIT PREPARED`, the coordinator must durably know that the global outcome is commit. Recovery after a crash should read that record and repeat the terminal command idempotently. PostgreSQL stores the participant's prepared state; it does not decide the business outcome for the coordinator.

Record at least:

- global operation ID and participant-specific GID;
- participant database and recovery credentials reference;
- prepare acknowledgement from every participant;
- durable global decision;
- completion acknowledgement from every participant;
- retry and escalation state.

### Alert on age, not merely count

A prepared transaction is expected to be short-lived. Monitor `pg_prepared_xacts.prepared` and alert when age exceeds the coordinator's recovery objective. A count of one may be healthy for a few milliseconds and dangerous for hours.

PostgreSQL warns that old prepared transactions retain locks, delay storage reclamation, and can contribute to transaction-ID wraparound pressure. The correct response is to recover the known decision, not to guess between commit and rollback.

### Resolve prepared work before deleting the sandbox

Database cleanup and prepared-transaction recovery are ordered operations:

1. identify the exact prepared GIDs for the sandbox database;
2. obtain the coordinator's durable commit or rollback decision;
3. finish each prepared transaction as its owner or an authorized operator;
4. verify `pg_prepared_xacts` has no rows for the database;
5. delete the PGSandbox database and role.

PostgreSQL can refuse database removal while prepared transactions remain. The [manual cleanup comparison](/blog/cleanup-expired-vs-manual-postgres-cleanup/) explains why connection termination alone does not resolve that blocker.

## Common two-phase commit test mistakes

### Enabling 2PC on an unrelated shared cluster

Use a dedicated profile. The test requires a server-start setting and creates deliberately in-doubt work. Do not change a shared development or production cluster to satisfy a local integration test.

### Using a constant GID

Prepared GIDs must be unique among active prepared transactions. A constant such as `test-transaction` creates collisions between parallel jobs and makes ownership ambiguous. Include a run identifier while keeping the prefix recognizable.

### Proving only `COMMIT PREPARED`

A coordinator has two terminal paths. Test rollback with the same rigor as commit, including post-decision visibility and catalog removal.

### Closing the connection and assuming rollback

The PostgreSQL [protocol overview](https://www.postgresql.org/docs/current/protocol-overview.html) says an ordinary incomplete transaction is rolled back when its connection ends. A prepared transaction has already detached from that session. Closing the original connection is part of the recovery test, not a cleanup mechanism.

### Treating one participant as a distributed-system proof

This harness proves PostgreSQL behavior for one participant. A complete coordinator test must involve every resource type, persist the global decision, inject failures between acknowledgements, and recover after process restart.

### Cleaning up by GID prefix alone

Prefixes help operators search, but they are not sufficient authorization. Match the durable coordinator record, database, owner, and exact GID before choosing commit or rollback.

## PR-ready two-phase commit proof

A useful agent-generated PR note should report:

```text
PostgreSQL 2PC proof
- profile: prepared-tx-test (database URL redacted)
- max_prepared_transactions: 10
- commit GID: unique test GID observed under sandbox role/database
- pre-decision visibility: neither prepared row visible
- commit decision: row 1 visible; GID removed
- rollback decision: row 2 absent; GID removed
- final prepared transactions created by test: 0
- sandbox cleanup: completed after recovery checks
```

Do not paste the sandbox connection string or role password into the PR. Keep the evidence structural: profile name, redacted database identity, configuration value, branch outcomes, remaining-GID count, and cleanup result.

The same evidence shape works for an MCP-driven agent. The agent can use the [PGSandbox MCP tool contract](/docs/mcp-tools/) to create and inspect the disposable database, then run the repository harness through the one-shot CLI session. The agent orchestrates the test inside a profile that an operator has already configured for prepared transactions.

## Frequently asked questions

### How do you test PostgreSQL two-phase commit?

Prepare a write under a unique GID, close the original connection, and inspect `pg_prepared_xacts` from another session. Prove the write stays invisible before the decision, then test `COMMIT PREPARED` and `ROLLBACK PREPARED` separately. Finish by asserting that every GID created by the test has disappeared.

### Why does PREPARE TRANSACTION say prepared transactions are disabled?

`max_prepared_transactions` is probably `0`, PostgreSQL's default. An operator must set it to a positive value and restart the server. Use a dedicated test cluster or profile, and enable the feature only when a transaction manager owns recovery.

### Does a prepared transaction roll back when its connection closes?

No. `PREPARE TRANSACTION` detaches the transaction from its original session. Another authorized session must issue `COMMIT PREPARED` or `ROLLBACK PREPARED`; closing the writer connection is not a terminal decision.

### How do you find prepared transactions in PostgreSQL?

Query `pg_prepared_xacts`. It reports each current prepared transaction's GID, preparation time, owner, and database. Recovery code should match the exact coordinator record and participant identity rather than acting on every row or a broad prefix.

### Can PGSandbox enable max_prepared_transactions?

No. PGSandbox uses PostgreSQL profiles that you configure, but it does not change cluster startup settings. Point the test at a dedicated profile whose server already has a deliberate positive `max_prepared_transactions` value.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Article",
      "headline": "How to Test PostgreSQL Two-Phase Commit",
      "description": "Prove prepare-state invisibility, pg_prepared_xacts identity, cross-session commit and rollback, and zero remaining prepared transactions.",
      "datePublished": "2026-08-02",
      "dateModified": "2026-08-02",
      "author": {"@type": "Organization", "name": "PGSandbox Team"},
      "publisher": {"@type": "Organization", "name": "PGSandbox MCP"},
      "mainEntityOfPage": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-two-phase-commit/"
    },
    {
      "@type": "HowTo",
      "name": "Test PostgreSQL two-phase commit",
      "description": "Verify PostgreSQL prepared-transaction configuration, preparation, identity, commit and rollback decisions, and cleanup.",
      "step": [
        {"@type": "HowToStep", "name": "Check configuration", "text": "Require max_prepared_transactions to be greater than zero on a dedicated test profile."},
        {"@type": "HowToStep", "name": "Prepare a transaction", "text": "Write inside a transaction, prepare it under a unique GID, and close the writer connection."},
        {"@type": "HowToStep", "name": "Inspect identity and visibility", "text": "Match the GID, owner, and database in pg_prepared_xacts while proving the row remains invisible."},
        {"@type": "HowToStep", "name": "Test both decisions", "text": "Commit one prepared transaction and roll back another from an autocommit recovery session."},
        {"@type": "HowToStep", "name": "Verify hygiene", "text": "Assert every GID created by the test is absent before deleting the disposable database."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do you test PostgreSQL two-phase commit?", "acceptedAnswer": {"@type": "Answer", "text": "Prepare a write under a unique GID, close the original connection, inspect pg_prepared_xacts from another session, prove pre-decision invisibility, test commit and rollback separately, and assert no test GIDs remain."}},
        {"@type": "Question", "name": "Why does PREPARE TRANSACTION say prepared transactions are disabled?", "acceptedAnswer": {"@type": "Answer", "text": "max_prepared_transactions is probably zero, PostgreSQL's default. An operator must set a positive value and restart a dedicated test server."}},
        {"@type": "Question", "name": "Does a prepared transaction roll back when its connection closes?", "acceptedAnswer": {"@type": "Answer", "text": "No. PREPARE TRANSACTION detaches work from the original session. An authorized session must issue COMMIT PREPARED or ROLLBACK PREPARED."}},
        {"@type": "Question", "name": "How do you find prepared transactions in PostgreSQL?", "acceptedAnswer": {"@type": "Answer", "text": "Query pg_prepared_xacts, which reports each current prepared transaction's GID, preparation time, owner, and database."}},
        {"@type": "Question", "name": "Can PGSandbox enable max_prepared_transactions?", "acceptedAnswer": {"@type": "Answer", "text": "No. PGSandbox selects configured PostgreSQL profiles but does not change cluster startup settings."}}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "Home", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test PostgreSQL Two-Phase Commit", "item": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-two-phase-commit/"}
      ]
    }
  ]
}
</script>
