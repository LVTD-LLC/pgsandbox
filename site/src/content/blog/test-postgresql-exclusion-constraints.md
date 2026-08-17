---
title: "How to Test PostgreSQL Exclusion Constraints"
excerpt: "Prove the installed operator contract, allowed and rejected ranges, update behavior, deferral timing, SQLSTATE 23P01, and disposable cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-08-17"
updatedAt: "2026-08-17T06:00:00Z"
tags: ["Postgres", "exclusion constraints", "range types", "integration testing", "coding agents"]
category: "Engineering"
metaTitle: "How to Test PostgreSQL Exclusion Constraints"
metaDescription: "Test PostgreSQL exclusion constraints with catalog, range-boundary, update, deferral, SQLSTATE 23P01, and cleanup checks."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgresql-exclusion-constraints/"
heroImageUrl: ""
featured: false
sortOrder: 160
---
Test PostgreSQL exclusion constraints with a six-part proof: installed definition, allowed controls, rejected conflicts, update behavior, deferral timing, and final cleanup. Apply the real migration in a disposable database, assert SQLSTATE `23P01`, and verify exact rows after every failure path.

A single overlapping-insert test is too weak. It can pass while the migration uses the wrong range bounds, omits the resource key, checks only inserts, or defers failures to a transaction boundary the application does not handle. The constraint's catalog definition and its behavioral edges are both part of the contract.

This guide packages those edges into an **Exclusion Boundary Proof**. PGSandbox MCP supplies a task-scoped database and role on your configured PostgreSQL server. PostgreSQL catalogs, deterministic timestamp ranges, transaction checkpoints, and sandbox deletion provide reviewable evidence.

*Published and last updated August 17, 2026.*

The workflow is:

1. Apply the real migration and inspect the exclusion constraint plus its supporting index.
2. Prove non-overlapping, adjacent, and different-resource rows are allowed.
3. Prove overlapping inserts and updates fail with SQLSTATE `23P01`.
4. Verify half-open range bounds at the exact shared endpoint.
5. If the constraint is deferrable, prove both repaired and unrepaired deferred conflicts.
6. Assert the committed final state and delete the disposable database.

## In this guide

- [Understand the exclusion rule](#what-a-postgresql-exclusion-constraint-test-must-prove)
- [Use the Exclusion Boundary Proof](#the-exclusion-boundary-proof)
- [Inspect the installed constraint](#1-inspect-the-installed-exclusion-constraint)
- [Run the deterministic harness](#2-run-a-deterministic-exclusion-constraint-test)
- [Test deferral separately](#3-test-the-deferral-boundary)
- [Run the proof with PGSandbox](#4-run-the-proof-in-a-disposable-database)
- [Record PR evidence](#pr-ready-exclusion-constraint-proof)
- [Answer common questions](#postgresql-exclusion-constraint-testing-faq)

## What a PostgreSQL exclusion constraint test must prove

An exclusion constraint compares every pair of rows with the operators declared in the constraint. PostgreSQL guarantees that at least one comparison is false or null. The current [constraint documentation](https://www.postgresql.org/docs/current/ddl-constraints.html#DDL-CONSTRAINTS-EXCLUSION) also notes that adding the constraint creates an index of the declared access method.

For room bookings, the usual rule is:

```sql
CREATE EXTENSION btree_gist;

CREATE TABLE room_bookings (
    booking_id bigint PRIMARY KEY,
    room_id integer NOT NULL,
    reserved_at tstzrange NOT NULL,
    CONSTRAINT room_bookings_no_overlap
        EXCLUDE USING gist (
            room_id WITH =,
            reserved_at WITH &&
        )
        DEFERRABLE INITIALLY IMMEDIATE
);
```

This blocks rows only when both comparisons are true: the room IDs are equal and the timestamp ranges overlap. A booking for another room remains valid. So does an adjacent half-open range such as `[10:00,11:00)` followed by `[11:00,12:00)`.

PostgreSQL's [range-type documentation](https://www.postgresql.org/docs/current/rangetypes.html#RANGETYPES-CONSTRAINT) uses the same combination of scalar equality and range overlap. The scalar `integer` equality operator needs a GiST operator class, which the supplied [`btree_gist` extension](https://www.postgresql.org/docs/current/btree-gist.html) provides. The extension is useful here because one multicolumn GiST index must compare both the scalar room ID and the range.

### Exclusion is not the same as uniqueness

A unique constraint asks whether selected values are equal. An exclusion constraint can use other commutative operators, including the range-overlap operator `&&`. PostgreSQL's [`CREATE TABLE` reference](https://www.postgresql.org/docs/current/sql-createtable.html) explains that each operator must belong to an operator class supported by the chosen index method; in practice exclusion constraints generally use GiST or SP-GiST.

The useful testing consequence is simple: duplicate-value fixtures are not enough. You need fixtures that exercise each operator independently and together.

| Fixture | Same room? | Overlapping range? | Expected |
| --- | ---: | ---: | --- |
| Adjacent booking | Yes | No | Allowed |
| Overlap in another room | No | Yes | Allowed |
| Contained booking | Yes | Yes | Rejected with `23P01` |
| Update into occupied time | Yes | Yes | Rejected with `23P01` |

### Range bounds are schema behavior

The constructors `tstzrange(start, end, '[)')` and `tstzrange(start, end, '[]')` do not describe the same booking policy. The first includes the start and excludes the end, so adjacent bookings do not overlap. The second includes both endpoints, so two bookings that share an endpoint conflict.

Do not let a test library choose bounds implicitly. Build the range explicitly or store a range column populated by the real application path. Then place one control row exactly on the existing upper bound. This catches a migration or adapter that silently changes `[)` into `[]`.

### Empty, unbounded, and partial ranges need policy tests

An overlap exclusion constraint does not make every range-shaped input valid for your domain. PostgreSQL accepts empty and unbounded ranges, and an empty range has no points to overlap. If an empty booking is invalid, add and test `CHECK (NOT isempty(reserved_at))`. If an open-ended booking is valid, prove exactly which later ranges it blocks.

| Edge | PostgreSQL behavior | Application proof |
| --- | --- | --- |
| `empty` | Contains no points, so it does not overlap another range | Reject with a separate `CHECK` if zero-length bookings are invalid |
| `[start,)` | Has no upper bound | Prove a later booking on the same resource conflicts |
| `(,end)` | Has no lower bound | Prove an earlier booking on the same resource conflicts |
| SQL `NULL` | Can make an exclusion comparison null | Keep participating columns `NOT NULL` when every row must be checked |

For a partial rule such as `WHERE (status = 'confirmed')`, add three controls: an overlapping pending row succeeds, an overlapping confirmed row fails, and updating pending to confirmed fails. The predicate controls which rows participate; it does not create a one-sided “conflicts with confirmed” operator.

Exclusion constraints are also unrelated to PostgreSQL's `constraint_exclusion` planner setting. `EXCLUDE` enforces a cross-row integrity rule. `constraint_exclusion` concerns whether the query planner can omit relations based on constraints.

## The Exclusion Boundary Proof

A reviewable exclusion-constraint test should answer six questions:

| Gate | Question | Stable evidence |
| --- | --- | --- |
| Declaration | What operators, expressions, predicate, access method, and timing did the migration install? | `pg_constraint`, `pg_get_constraintdef()`, and the supporting index |
| Allowed controls | Do adjacent ranges and the same range on another resource succeed? | Exact committed control rows |
| Conflict | Do overlapping inserts fail for the same resource? | Exception class plus SQLSTATE `23P01` |
| Mutation | Can an existing row be updated into a conflict? | Rejected `UPDATE` and unchanged row |
| Timing | If deferrable, does a repaired conflict commit while an unrepaired one fails at validation? | `SET CONSTRAINTS` checkpoints and exact final rows |
| Cleanup | Did the proof remove the task database and role? | Credential-free sandbox deletion result |

The information gain is the **operator truth-table plus timing proof**. Basic examples often stop at one rejected booking. This proof isolates both operator dimensions, the exact range boundary, insert and update paths, and immediate versus deferred validation. That is enough evidence to review the migration as a database contract rather than as a lucky example.

## 1. Inspect the installed exclusion constraint

Apply the repository's real migration before running assertions. Recreating similar DDL inside the test proves the example, not the migration under review. The [database migration testing workflow](/blog/database-migration-testing-agent-pr/) covers that boundary.

PostgreSQL records exclusion constraints in [`pg_constraint`](https://www.postgresql.org/docs/current/catalog-pg-constraint.html) with `contype = 'x'`. The catalog also stores whether the constraint is deferrable, its initial timing, validation status, and the OID of its supporting index. PGSandbox's schema description and schema-diff workflows classify this object as an exclusion constraint, so use those for compact migration evidence and keep the direct catalog assertion in the executable test.

```sql
SELECT
    c.conname,
    c.contype,
    c.condeferrable,
    c.condeferred,
    c.convalidated,
    pg_get_constraintdef(c.oid, true) AS definition,
    i.relname AS supporting_index,
    am.amname AS access_method
FROM pg_constraint AS c
JOIN pg_class AS t ON t.oid = c.conrelid
JOIN pg_namespace AS n ON n.oid = t.relnamespace
JOIN pg_class AS i ON i.oid = c.conindid
JOIN pg_am AS am ON am.oid = i.relam
WHERE n.nspname = 'public'
  AND t.relname = 'room_bookings'
  AND c.conname = 'room_bookings_no_overlap';
```

Assert semantic fields, not a byte-for-byte deparse. For the example above, require one row with `contype = 'x'`, `condeferrable = true`, `condeferred = false`, `convalidated = true`, and `access_method = 'gist'`. Normalize the definition and check that it contains `room_id WITH =`, `reserved_at WITH &&`, and the intended timing.

If the migration uses a partial exclusion constraint, assert its predicate too. A predicate such as `WHERE (status = 'confirmed')` deliberately excludes other rows from the index-backed rule. That may be correct, but it is a different contract from “no row may overlap a confirmed row.” Exclusion operators are symmetric; a one-sided status rule often needs separate application validation.

## 2. Run a deterministic exclusion-constraint test

The following Psycopg 3 harness uses fixed UTC timestamps, explicit half-open bounds, and separate transactions for each expected failure. Psycopg's [transaction documentation](https://www.psycopg.org/psycopg3/docs/basic/transactions.html) defines the `autocommit=True` plus explicit `transaction()` behavior used to isolate and roll back each expected error. Its [errors API](https://www.psycopg.org/psycopg3/docs/api/errors.html) documents `ExclusionViolation`, SQLSTATE, and server diagnostics. In a real application repository, replace `install_fixture()` with the actual migration command and keep the catalog and behavior assertions.

```python
import os

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]
CONSTRAINT = "room_bookings_no_overlap"
SET_CONSTRAINT = "public.room_bookings_no_overlap"


def install_fixture(conn):
    # btree_gist must already be installed by the database lifecycle/admin.
    conn.execute("DROP TABLE IF EXISTS room_bookings")
    conn.execute(
        """
        CREATE TABLE room_bookings (
            booking_id bigint PRIMARY KEY,
            room_id integer NOT NULL,
            reserved_at tstzrange NOT NULL,
            CONSTRAINT room_bookings_no_overlap
                EXCLUDE USING gist (
                    room_id WITH =,
                    reserved_at WITH &&
                )
                DEFERRABLE INITIALLY IMMEDIATE
        )
        """
    )


def insert_booking(conn, booking_id, room_id, start, end):
    conn.execute(
        """
        INSERT INTO room_bookings
            (booking_id, room_id, reserved_at)
        VALUES (%s, %s, tstzrange(%s, %s, '[)'))
        """,
        (booking_id, room_id, start, end),
    )


def expect_exclusion_violation(operation):
    try:
        operation()
    except errors.ExclusionViolation as exc:
        assert exc.sqlstate == "23P01"
        assert exc.diag.constraint_name == CONSTRAINT
        return
    raise AssertionError("expected exclusion constraint violation")


with psycopg.connect(DATABASE_URL, autocommit=True) as conn:
    install_fixture(conn)

    declaration = conn.execute(
        """
        SELECT
            c.contype,
            c.condeferrable,
            c.condeferred,
            c.convalidated,
            am.amname,
            pg_get_constraintdef(c.oid, true)
        FROM pg_constraint AS c
        JOIN pg_class AS i ON i.oid = c.conindid
        JOIN pg_am AS am ON am.oid = i.relam
        WHERE c.conrelid = 'room_bookings'::regclass
          AND c.conname = 'room_bookings_no_overlap'
        """
    ).fetchone()
    assert declaration[:5] == ("x", True, False, True, "gist")
    assert "room_id WITH =" in declaration[5]
    assert "reserved_at WITH &&" in declaration[5]

    # Baseline: room 101 is reserved from 10:00 inclusive to 11:00 exclusive.
    insert_booking(
        conn, 1, 101,
        "2026-08-17 10:00:00+00",
        "2026-08-17 11:00:00+00",
    )

    # Positive controls isolate each operator.
    insert_booking(
        conn, 2, 101,
        "2026-08-17 11:00:00+00",
        "2026-08-17 12:00:00+00",
    )  # Same room, adjacent [) range: allowed.
    insert_booking(
        conn, 3, 202,
        "2026-08-17 10:30:00+00",
        "2026-08-17 10:45:00+00",
    )  # Different room, overlapping range: allowed.

    # Same room plus overlap must fail with the stable SQLSTATE.
    def overlapping_insert():
        with conn.transaction():
            insert_booking(
                conn, 4, 101,
                "2026-08-17 10:30:00+00",
                "2026-08-17 10:45:00+00",
            )

    expect_exclusion_violation(overlapping_insert)

    # UPDATE is a separate write path and needs its own proof.
    def overlapping_update():
        with conn.transaction():
            conn.execute(
                """
                UPDATE room_bookings
                SET room_id = 101,
                    reserved_at = tstzrange(
                        '2026-08-17 10:15:00+00',
                        '2026-08-17 10:30:00+00',
                        '[)'
                    )
                WHERE booking_id = 3
                """
            )

    expect_exclusion_violation(overlapping_update)

    rows = conn.execute(
        """
        SELECT
            booking_id,
            room_id,
            lower(reserved_at)::text,
            upper(reserved_at)::text,
            lower_inc(reserved_at),
            upper_inc(reserved_at)
        FROM room_bookings
        ORDER BY booking_id
        """
    ).fetchall()
    assert rows == [
        (1, 101, "2026-08-17 10:00:00+00", "2026-08-17 11:00:00+00", True, False),
        (2, 101, "2026-08-17 11:00:00+00", "2026-08-17 12:00:00+00", True, False),
        (3, 202, "2026-08-17 10:30:00+00", "2026-08-17 10:45:00+00", True, False),
    ]
```

PostgreSQL publishes `23P01` as `exclusion_violation` in its current [error-code appendix](https://www.postgresql.org/docs/current/errcodes-appendix.html). Assert the SQLSTATE and constraint name, not the localized message text. This produces a stable application and review contract.

The positive controls matter as much as the rejected row. If booking 2 fails, inspect the bounds and declared operators first. If booking 3 fails, inspect the resource-equality dimension.

## 3. Test the deferral boundary

Only test deferral when the migration declares the exclusion constraint `DEFERRABLE`. PostgreSQL's [`SET CONSTRAINTS` reference](https://www.postgresql.org/docs/current/sql-set-constraints.html) says immediate constraints are checked at the end of each statement, while deferred constraints wait until transaction commit. Switching from deferred to immediate checks outstanding changes retroactively.

Add two paths to the same harness:

```python
# A temporary conflict is valid only because this transaction repairs it.
with conn.transaction():
    conn.execute(f"SET CONSTRAINTS {SET_CONSTRAINT} DEFERRED")
    insert_booking(
        conn, 5, 101,
        "2026-08-17 10:20:00+00",
        "2026-08-17 10:40:00+00",
    )
    conn.execute(
        """
        UPDATE room_bookings
        SET reserved_at = tstzrange(
            '2026-08-17 12:00:00+00',
            '2026-08-17 13:00:00+00',
            '[)'
        )
        WHERE booking_id = 5
        """
    )
    conn.execute(f"SET CONSTRAINTS {SET_CONSTRAINT} IMMEDIATE")


def unrepaired_deferred_conflict():
    with conn.transaction():
        conn.execute(f"SET CONSTRAINTS {SET_CONSTRAINT} DEFERRED")
        insert_booking(
            conn, 6, 101,
            "2026-08-17 10:20:00+00",
            "2026-08-17 10:40:00+00",
        )
        # The exception is raised when the transaction exits and commits.


expect_exclusion_violation(unrepaired_deferred_conflict)

final_rows = conn.execute(
    """
    SELECT
        booking_id,
        room_id,
        lower(reserved_at)::text,
        upper(reserved_at)::text
    FROM room_bookings
    ORDER BY booking_id
    """
).fetchall()
assert final_rows == [
    (1, 101, "2026-08-17 10:00:00+00", "2026-08-17 11:00:00+00"),
    (2, 101, "2026-08-17 11:00:00+00", "2026-08-17 12:00:00+00"),
    (3, 202, "2026-08-17 10:30:00+00", "2026-08-17 10:45:00+00"),
    (5, 101, "2026-08-17 12:00:00+00", "2026-08-17 13:00:00+00"),
]
```

This separates three facts that a broad happy-path test can blur:

- The declared initial mode is immediate.
- A transaction can opt into deferred checking and repair a temporary conflict.
- An unrepaired conflict still fails with `23P01`, and its row does not commit.

It also complements the broader [deferrable-constraint testing guide](/blog/test-postgres-deferrable-constraints/). That guide covers timing across several constraint types; this proof adds the exclusion-specific operator and range boundaries.

## 4. Run the proof in a disposable database

Exclusion tests deliberately create invalid writes and failed transactions. Run them in a fresh task database instead of a shared development database. Follow the [PGSandbox MCP tool contract](/docs/mcp-tools/) if an MCP client owns the lifecycle, or use the one-shot CLI wrapper for a repository test command.

The example needs `btree_gist`. PGSandbox installs requested extensions through its admin lifecycle only when the selected profile explicitly allows them. Do not give the sandbox role extension-management authority. Create or update a PostgreSQL 18 profile, add `btree_gist` to that profile's `allowedExtensions`, verify it with `list_profiles`, then request it during provisioning:

```bash
pgsandbox with-database \
  --profile YOUR_PROFILE_NAME \
  --extension btree_gist \
  --ttl-minutes 30 \
  --cleanup always \
  --timeout-seconds 60 \
  --result-format json \
  -- uv run --with 'psycopg[binary]' \
     python tests/postgres_exclusion_constraint_proof.py
```

PGSandbox injects `PGSANDBOX_DATABASE_URL` and `DATABASE_URL` into the child process. It creates the database and scoped role, installs the allowlisted extension with lifecycle authority, runs the test, redacts generated credentials from bounded output, and applies the chosen cleanup policy.

For CI and unattended coding-agent runs, use `--cleanup always`. During active diagnosis, `--cleanup on-success` keeps a failing sandbox long enough to inspect it, still bounded by its TTL. The [disposable integration-test guide](/blog/run-integration-tests-disposable-postgres-database/) explains those session result and retention choices.

If provisioning returns `extension_not_allowed`, change the profile policy rather than bypassing it with broader task credentials. `invalid_extensions` means the requested extension is unavailable on that target, so install the appropriate PostgreSQL contrib package or choose a compatible server. `extension_setup_required` means the returned hint identified required server-level configuration, packaging, or preload setup. PGSandbox does not provision a new server per sandbox or operate a hosted PostgreSQL service; it can install supported local binaries and manage its local cluster.

## PR-ready exclusion constraint proof

Keep the review artifact compact and credential-free:

```json
{
  "constraint": "public.room_bookings_no_overlap",
  "accessMethod": "gist",
  "operators": ["room_id =", "reserved_at &&"],
  "bounds": "[)",
  "timing": "DEFERRABLE INITIALLY IMMEDIATE",
  "allowedControls": ["adjacent_same_room", "overlap_different_room"],
  "rejectedPaths": {
    "insert": "23P01",
    "update": "23P01",
    "deferredCommit": "23P01"
  },
  "repairedDeferredConflict": "committed",
  "finalBookingIds": [1, 2, 3, 5],
  "cleanup": "deleted"
}
```

This is stronger than attaching raw logs. It tells a reviewer what the migration installed, which controls isolate each operator, where failures occur, and what data survived. Keep the exact test command and migration revision beside the JSON, but omit URLs and passwords.

## PostgreSQL exclusion constraint testing FAQ

### What SQLSTATE does an exclusion constraint violation use?

PostgreSQL uses SQLSTATE `23P01`, named `exclusion_violation`. Test that stable code and the reported constraint name. Do not assert the full error message, which can vary by server version, driver, and locale.

### Why does a room ID need `btree_gist`?

Range types have native GiST support for operators such as overlap (`&&`). Plain scalar types such as `integer`, `text`, and `uuid` need GiST operator classes when they share the multicolumn exclusion index. `btree_gist` supplies B-tree-equivalent GiST behavior for those scalar types.

### Should adjacent timestamp ranges conflict?

That depends on the bounds. With half-open `[start,end)` ranges, one booking may begin exactly when another ends. With inclusive upper bounds, the shared endpoint overlaps. Make the constructor bounds explicit and include an exact-endpoint control in the test.

### Do exclusion constraints apply to updates?

Yes. An update that changes an indexed expression into a conflict must be rejected just like an insert. Test the update path separately because application adapters and migration assumptions can differ between creates and edits.

### Can an exclusion constraint be deferred?

Yes, when declared `DEFERRABLE`. A transaction may switch it to deferred, hold a temporary conflict, repair it, and validate before commit. An unrepaired conflict must still fail at `SET CONSTRAINTS ... IMMEDIATE` or commit with SQLSTATE `23P01`.

### Should I test this on a shared development database?

No. The proof intentionally triggers integrity errors and transaction rollbacks, and it may need `btree_gist`. Use a disposable database with a scoped role, explicit extension policy, bounded retention, and cleanup evidence. PGSandbox can provide that lifecycle on a PostgreSQL server you already operate.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "FAQPage",
  "mainEntity": [
    {
      "@type": "Question",
      "name": "What SQLSTATE does an exclusion constraint violation use?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "PostgreSQL uses SQLSTATE 23P01, named exclusion_violation. Tests should assert that stable code and the constraint name instead of the complete error message."
      }
    },
    {
      "@type": "Question",
      "name": "Why does a room ID need btree_gist?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "Range types have native GiST support for overlap. btree_gist supplies GiST operator classes for scalar types such as integer, text, and uuid so a multicolumn exclusion index can compare the resource key and the range."
      }
    },
    {
      "@type": "Question",
      "name": "Should adjacent timestamp ranges conflict?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "It depends on the range bounds. Half-open [start,end) ranges allow one booking to begin when another ends, while an inclusive upper bound makes that shared endpoint overlap."
      }
    },
    {
      "@type": "Question",
      "name": "Do exclusion constraints apply to updates?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "Yes. PostgreSQL rejects an update that changes an indexed expression into a conflict, so tests should cover updates separately from inserts."
      }
    },
    {
      "@type": "Question",
      "name": "Can an exclusion constraint be deferred?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "Yes, when it is declared DEFERRABLE. A transaction may temporarily hold and repair a conflict, but an unrepaired conflict must fail during an immediate validation checkpoint or at commit with SQLSTATE 23P01."
      }
    },
    {
      "@type": "Question",
      "name": "Should I test this on a shared development database?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "No. Use a disposable database with a scoped role, explicit extension policy, bounded retention, and cleanup evidence because the proof intentionally triggers integrity errors and rollbacks."
      }
    }
  ]
}
</script>
