---
title: "How to Test PostgreSQL LISTEN/NOTIFY Workflows"
excerpt: "Prove registration, commit delivery, rollback silence, payload identity, duplicate folding, bounded waits, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-24"
updatedAt: "2026-08-24T06:00:00Z"
tags: ["Postgres", "LISTEN NOTIFY", "integration testing", "Psycopg", "coding agents"]
category: "Engineering"
metaTitle: "How to Test PostgreSQL LISTEN/NOTIFY Workflows"
metaDescription: "Test PostgreSQL LISTEN/NOTIFY with commit, rollback, payload, duplicate-folding, timeout, and disposable cleanup checks."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/test-postgresql-listen-notify-workflows/"
heroImageUrl: ""
featured: false
sortOrder: 161
---
Test PostgreSQL `LISTEN`/`NOTIFY` workflows with two independent connections and explicit transaction checkpoints. Register the listener before sending, prove that committed notifications arrive, prove that rolled-back notifications do not, assert the channel and payload, test duplicate folding, and put a deadline around every wait.

A test that sends one notification and prints what it receives is too weak. It can pass while the listener registration races the sender, the application publishes before commit, a rollback leaks an event, or the test hangs forever when delivery fails. This guide turns those failure modes into a **Notification Delivery Proof** that a coding agent can run in a disposable database and summarize in a PR.

PGSandbox MCP supplies the task-scoped database and role. PostgreSQL supplies the transactional notification semantics. Psycopg 3 supplies a bounded notification iterator, so the test can fail on a deadline instead of sleeping and hoping.

*Published and last updated August 24, 2026.*

The workflow is:

1. Open a dedicated listener connection and commit `LISTEN` before publishing.
2. Send a unique payload inside an uncommitted transaction and prove early silence.
3. Commit and assert the exact channel, payload, and sender process ID.
4. Roll back another notification and prove it is never delivered.
5. Test same-payload folding and distinct-payload delivery separately.
6. Bound every receive call, record the result, and delete the disposable database.

## In this guide

- [Understand the transaction contract](#what-a-postgresql-listen-notify-test-must-prove)
- [Use the Notification Delivery Proof](#the-notification-delivery-proof)
- [Run the deterministic harness](#run-a-deterministic-psycopg-listen-notify-test)
- [Test trigger-driven notifications](#test-the-real-trigger-or-publisher-path)
- [Run it in a disposable database](#run-the-proof-with-pgsandbox)
- [Handle queue and payload limits](#test-operational-boundaries-separately)
- [Record review evidence](#pr-ready-listen-notify-proof)
- [Answer common questions](#postgresql-listen-notify-testing-faq)

## What a PostgreSQL LISTEN/NOTIFY test must prove

PostgreSQL notifications are transactional signals between database sessions. `LISTEN` registers the current session on a channel. `NOTIFY` sends a channel name plus an optional payload to sessions already listening on that channel.

Both commands have transaction boundaries that matter in tests. The PostgreSQL [`LISTEN` reference](https://www.postgresql.org/docs/current/sql-listen.html) says a registration takes effect at transaction commit. The [`NOTIFY` reference](https://www.postgresql.org/docs/current/sql-notify.html) says a notification issued inside a transaction is delivered only if that transaction commits. A listening client also receives pending events only after its own current transaction ends.

That creates an easy race:

```text
listener: LISTEN task_events        sender: NOTIFY task_events
listener: COMMIT
```

If the sender commits before the listener's registration commit, the listener is not entitled to receive that event. A sleep makes the race less frequent; it does not prove the ordering. The test must establish the listener synchronously, then let the sender proceed.

### LISTEN/NOTIFY is a signal, not a durable event log

`NOTIFY` tells active listeners that something happened. It does not provide replay to a later listener, consumer acknowledgements, or durable per-consumer offsets. The PostgreSQL documentation recommends a safe startup sequence: commit `LISTEN`, inspect the authoritative database state in a new transaction, then use notifications for subsequent changes. The first notifications may describe changes the initial state read already observed, so consumers should tolerate that overlap.

For application design, put durable facts in tables and treat a notification as a prompt to read them. If delivery must survive disconnected consumers and support replay, test the table-backed recovery path as well as the live notification path.

### The listener should own a physical connection

`LISTEN` is session state, so the listening code needs a stable PostgreSQL session. Do not register on an arbitrary pooled connection and return it to the pool. Another request can borrow that session, or the pool can replace it, while the application still believes it is subscribed.

Psycopg's [asynchronous notification guidance](https://www.psycopg.org/psycopg3/docs/advanced/async.html#asynchronous-notifications) recommends autocommit for timely notification handling. Its `Connection.notifies()` generator supports `timeout` and `stop_after`, which are useful test boundaries. Use either that generator or notification handlers consistently; current Psycopg guidance warns against mixing both styles.

## The Notification Delivery Proof

A reviewable PostgreSQL `LISTEN`/`NOTIFY` test should answer six questions:

| Gate | Question | Stable evidence |
| --- | --- | --- |
| Registration | Was `LISTEN` effective before any sender transaction committed? | Dedicated autocommit listener plus a completed `LISTEN` command |
| Commit | Is a notification invisible before commit and delivered after commit? | Empty bounded pre-commit receive, then one exact post-commit notification |
| Rollback | Does aborting the publisher transaction suppress delivery? | Empty bounded receive after `ROLLBACK` |
| Identity | Did the expected sender publish the expected event? | Exact channel, payload, and backend PID |
| Multiplicity | Are identical events folded while distinct payloads remain distinct? | One identical-payload event and two distinct-payload events |
| Cleanup | Does the test stop listening and remove its task database? | Closed connection, bounded process exit, and sandbox deletion result |

The information gain is the **transaction-and-multiplicity proof**, not another syntax example. Many examples demonstrate `LISTEN` in one terminal and `NOTIFY` in another. This contract proves the timing guarantees application code actually depends on and gives every negative path a deadline.

## Run a deterministic Psycopg LISTEN/NOTIFY test

The harness below uses a unique channel and unique payloads, two physical connections, and bounded receives. It does not use `time.sleep()`. Run it against a disposable PostgreSQL database through `PGSANDBOX_DATABASE_URL` or a test-only `DATABASE_URL`.

```python
import os
import uuid

import psycopg
from psycopg import sql


DATABASE_URL = os.environ.get("PGSANDBOX_DATABASE_URL") or os.environ["DATABASE_URL"]
CHANNEL = f"pgsandbox_notify_{uuid.uuid4().hex}"


def receive(conn, *, timeout=0.35, stop_after=None):
    """Collect notifications until the count or timeout boundary is reached."""
    return list(conn.notifies(timeout=timeout, stop_after=stop_after))


with (
    psycopg.connect(DATABASE_URL, autocommit=True) as listener,
    psycopg.connect(DATABASE_URL) as sender,
):
    # LISTEN is session state. Autocommit makes registration effective before
    # the sender is allowed to publish.
    listener.execute(
        sql.SQL("LISTEN {}").format(sql.Identifier(CHANNEL))
    )

    committed_payload = f"committed:{uuid.uuid4().hex}"
    sender.execute("SELECT pg_notify(%s, %s)", (CHANNEL, committed_payload))

    # A NOTIFY inside an open transaction must not be delivered yet.
    assert receive(listener) == []

    sender_pid = sender.info.backend_pid
    sender.commit()
    delivered = receive(listener, timeout=2.0, stop_after=1)
    assert len(delivered) == 1
    assert delivered[0].channel == CHANNEL
    assert delivered[0].payload == committed_payload
    assert delivered[0].pid == sender_pid

    # A rolled-back publisher transaction must stay silent.
    rolled_back_payload = f"rolled-back:{uuid.uuid4().hex}"
    sender.execute("SELECT pg_notify(%s, %s)", (CHANNEL, rolled_back_payload))
    sender.rollback()
    assert receive(listener) == []

    # PostgreSQL folds identical channel+payload events in one transaction.
    folded_payload = f"folded:{uuid.uuid4().hex}"
    sender.execute("SELECT pg_notify(%s, %s)", (CHANNEL, folded_payload))
    sender.execute("SELECT pg_notify(%s, %s)", (CHANNEL, folded_payload))
    sender.commit()
    folded = receive(listener, timeout=1.0)
    assert [(event.channel, event.payload) for event in folded] == [
        (CHANNEL, folded_payload)
    ]

    # Distinct payloads in the same transaction remain distinct events.
    first = f"distinct-a:{uuid.uuid4().hex}"
    second = f"distinct-b:{uuid.uuid4().hex}"
    sender.execute("SELECT pg_notify(%s, %s)", (CHANNEL, first))
    sender.execute("SELECT pg_notify(%s, %s)", (CHANNEL, second))
    sender.commit()
    distinct = receive(listener, timeout=2.0, stop_after=2)
    assert [event.payload for event in distinct] == [first, second]

    listener.execute(
        sql.SQL("UNLISTEN {}").format(sql.Identifier(CHANNEL))
    )

print(
    {
        "registration": "proved",
        "commit_delivery": "proved",
        "rollback_silence": "proved",
        "payload_identity": "proved",
        "duplicate_folding": "proved",
        "bounded_waits": "proved",
    }
)
```

The channel is composed with `psycopg.sql.Identifier` because identifiers cannot be passed as normal query parameters. Payloads and the channel argument to `pg_notify()` are values, so those use `%s` parameters.

The pre-commit and rollback assertions are deliberately short. They prove that no event appears within a bounded negative window. The positive receive gets a longer deadline because it must wait for network and client scheduling. An outer test-process timeout remains necessary in case the driver or environment fails outside those receive calls.

### Why test duplicate folding?

PostgreSQL can fold repeated notifications with the same channel and identical payload inside one transaction into a single delivered event. Distinct payloads remain distinct, and notifications from different transactions are not folded together. These rules are documented in the [`NOTIFY` notes](https://www.postgresql.org/docs/current/sql-notify.html).

Do not use notification count as a row-change count unless the publisher contract guarantees unique payloads and one event per transaction. A safer consumer reads durable state using the payload as a key, cursor, or invalidation hint.

## Test the real trigger or publisher path

The direct `pg_notify()` harness proves PostgreSQL and driver behavior. It does not prove that the application's trigger, outbox writer, or repository method publishes the right event.

Add one application-level case that:

1. Applies the repository's real migration.
2. Registers the listener and commits registration.
3. Performs the real write through the same code path production uses.
4. Commits the write.
5. Asserts the channel and a schema-valid payload.
6. Reads the changed row and verifies the payload points to that durable state.

For a trigger, inspect the installed definition before testing behavior. The [PostgreSQL trigger testing guide](/blog/test-postgresql-triggers/) shows how to verify catalog declaration, firing paths, side effects, rollback, and cleanup. Keep the transport proof and publisher proof separate: when one fails, you should know whether PostgreSQL delivery or application wiring broke.

Payload assertions should be semantic. Parse JSON and assert required fields, types, version, and identifiers rather than comparing whitespace or object-key order. Include a schema version if independent deploys may change producers and consumers at different times.

## Run the proof with PGSandbox

Use one child process so it can own both PostgreSQL connections for the full test. Separate PGSandbox `run_sql` calls are short-lived operations and cannot represent a stable listening session across transaction boundaries.

The [one-shot integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) explains the complete session contract. For this proof:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 30 \
  --timeout-seconds 120 \
  --cleanup always \
  --result-format json \
  -- uv run --with "psycopg[binary]" python tests/listen_notify_proof.py
```

PGSandbox creates a fresh database and scoped login role, injects `PGSANDBOX_DATABASE_URL` and standard libpq variables into the child process, bounds its output, and removes the database according to the cleanup policy. The application test never needs the admin connection that created the sandbox. See the [MCP tool contract](/docs/mcp-tools/) and [resource model](/docs/architecture/) for that authority boundary.

Use `--cleanup on-success` temporarily when a failed test needs database inspection. Keep a TTL as the recovery backstop, and never print the injected connection URL in test output or PR evidence.

## Test operational boundaries separately

Correct delivery in one integration test does not prove production capacity or durability. Keep operational checks separate from the functional harness.

### Payload size

PostgreSQL's default configuration requires a `NOTIFY` payload to be shorter than 8,000 bytes. Large data belongs in a table; send a compact identifier in the notification. Add a boundary test only if your publisher builds payloads near that limit. Otherwise, validate the application payload against a much smaller product-owned cap.

### Notification queue health

PostgreSQL stores notifications until all listening sessions have processed them. The standard queue is large, but a listener that enters a long transaction can prevent cleanup. The `pg_notification_queue_usage()` function reports the occupied fraction, and PostgreSQL logs warnings when queue pressure grows.

Monitor queue usage in operations, but do not make a normal unit or integration test fill the queue. Test the application response to a simulated queue-pressure alert separately. Keep listening connections out of long-running transactions.

### Disconnect and recovery

A reconnect creates a new session and must execute `LISTEN` again. Test reconnect behavior at the client-library layer: close the listener connection, establish a new one, register before declaring readiness, reload durable state, then resume live notification handling. Do not claim exactly-once delivery. Make processing idempotent and let the database table remain authoritative.

## Common PostgreSQL LISTEN/NOTIFY testing mistakes

### Publishing before registration commits

This creates a real missed-event window. Execute `LISTEN` on the dedicated connection, let that command commit, and only then release the sender. A barrier or sequential setup proves the order; a sleep does not.

### Keeping the listener in a transaction

PostgreSQL delivers notifications to a listening client between transactions. A listener that begins a transaction and leaves it open can delay reception and hold notification-queue cleanup. Use an autocommit listener unless the application has a specific, tested reason not to.

### Waiting without a timeout

An unbounded `next(conn.notifies())` can hang CI forever. Give every negative and positive receive a deadline, then add a longer outer process timeout. The [statement-timeout and cancellation guide](/blog/test-postgres-statement-timeouts-query-cancellation/) explains why server, driver, and process deadlines prove different boundaries.

### Testing only the happy path

Commit delivery alone does not prove transactional behavior. Rollback silence is the most important negative control. Duplicate folding and distinct-payload delivery catch consumers that mistake notifications for a durable row-by-row queue.

### Returning the listener to a pool

The registration belongs to one session, not to an abstract pool. Reserve a physical connection, monitor it, re-register after reconnect, and close or `UNLISTEN` it during shutdown.

## PR-ready LISTEN/NOTIFY proof

Keep credentials and raw connection strings out of the PR. Record the test contract instead:

```text
PostgreSQL LISTEN/NOTIFY proof
- target: disposable PostgreSQL 18 database
- registration: LISTEN effective before sender release
- commit: exact channel, payload, and sender PID received
- rollback: no event within bounded negative window
- multiplicity: identical payload folded; distinct payloads preserved
- publisher: real trigger/repository path verified separately
- deadlines: bounded receives plus 120-second outer timeout
- cleanup: sandbox database and role deleted
```

If the test fails, retain the database only when its durable state helps diagnosis. Notification registrations disappear with the client session, so a retained database cannot preserve an in-memory delivery failure. Capture sanitized assertion output and the publisher transaction result before the process exits.

## PostgreSQL LISTEN/NOTIFY testing FAQ

### How do you test PostgreSQL LISTEN/NOTIFY reliably?

Use two physical connections. Commit `LISTEN` on a dedicated autocommit listener before the sender starts, issue `pg_notify()` inside a separate transaction, prove silence before commit, commit, and assert the exact notification. Add rollback, duplicate-folding, timeout, and cleanup cases.

### Are PostgreSQL notifications delivered before commit?

No. A notification issued inside a transaction is delivered only after that transaction commits. If the transaction rolls back, the notification is not delivered. A listening client also receives pending notifications only after its own current transaction ends.

### Can PostgreSQL LISTEN/NOTIFY lose messages?

It is not a durable replay log. A session must already be listening when the publisher commits, and reconnecting requires a new `LISTEN`. Store durable state in tables, use notifications as wake-up hints, and reload authoritative state when a listener starts or reconnects.

### Does PostgreSQL deliver duplicate notifications?

PostgreSQL may fold identical channel-and-payload notifications issued in the same transaction into one event. Distinct payloads remain distinct, and events from different transactions are not folded together. Consumers should not infer changed-row counts from notification counts.

### Should a LISTEN connection use autocommit?

Usually yes. `LISTEN` becomes effective at commit, and notifications are delivered to the client between transactions. A dedicated autocommit connection avoids accidental registration and delivery delays caused by an open transaction.

### Can PGSandbox `run_sql` hold a listener open?

No reliable multi-step listening workflow should depend on separate `run_sql` calls. Run a bounded child test process through `pgsandbox with-database`; let that process own both physical connections while PGSandbox owns provisioning, credential injection, supervision, and cleanup.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Test PostgreSQL LISTEN/NOTIFY workflows",
      "description": "Prove listener registration, commit delivery, rollback silence, payload identity, duplicate folding, bounded waiting, and cleanup.",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Register the listener", "text": "Open a dedicated autocommit connection and execute LISTEN before the sender starts."},
        {"@type": "HowToStep", "position": 2, "name": "Prove transaction timing", "text": "Issue pg_notify in a separate transaction, verify silence before commit, then commit and receive the event."},
        {"@type": "HowToStep", "position": 3, "name": "Assert event identity", "text": "Check the exact channel, payload, and sender backend PID."},
        {"@type": "HowToStep", "position": 4, "name": "Prove rollback silence", "text": "Roll back a second publisher transaction and verify no event arrives within a bounded window."},
        {"@type": "HowToStep", "position": 5, "name": "Test multiplicity", "text": "Verify identical payload folding and distinct-payload delivery in separate cases."},
        {"@type": "HowToStep", "position": 6, "name": "Clean up", "text": "Stop listening, close both connections, and delete the disposable database and role."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do you test PostgreSQL LISTEN/NOTIFY reliably?", "acceptedAnswer": {"@type": "Answer", "text": "Use two physical connections, commit LISTEN before publishing, prove pre-commit silence and post-commit delivery, then add rollback, duplicate-folding, timeout, and cleanup checks."}},
        {"@type": "Question", "name": "Are PostgreSQL notifications delivered before commit?", "acceptedAnswer": {"@type": "Answer", "text": "No. Notifications issued inside a transaction are delivered only after commit and are suppressed by rollback."}},
        {"@type": "Question", "name": "Can PostgreSQL LISTEN/NOTIFY lose messages?", "acceptedAnswer": {"@type": "Answer", "text": "LISTEN/NOTIFY is not a durable replay log. Listeners must already be registered, so durable state should live in tables and be reloaded after startup or reconnect."}},
        {"@type": "Question", "name": "Does PostgreSQL deliver duplicate notifications?", "acceptedAnswer": {"@type": "Answer", "text": "PostgreSQL may fold identical channel-and-payload events in one transaction. Distinct payloads and events from different transactions remain distinct."}},
        {"@type": "Question", "name": "Should a LISTEN connection use autocommit?", "acceptedAnswer": {"@type": "Answer", "text": "Usually yes. Autocommit makes registration effective immediately and avoids delaying notifications behind a long-running listener transaction."}}
      ]
    }
  ]
}
</script>
