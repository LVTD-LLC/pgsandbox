---
title: "How to Test Postgres Row-Level Security in a Disposable Database"
excerpt: "Prove that Postgres RLS filters tenant reads, rejects cross-tenant writes, applies to the table owner, resets context, and survives cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-28"
updatedAt: "2026-07-28T06:00:00Z"
tags: ["Postgres", "row-level security", "RLS testing", "multi-tenant databases", "coding agents"]
category: "Engineering"
metaTitle: "Test Postgres Row-Level Security in a Sandbox"
metaDescription: "Test Postgres row-level security with FORCE RLS, tenant read and write checks, transaction-scoped context, and disposable database cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-row-level-security/"
heroImageUrl: ""
featured: false
sortOrder: 150
---
Test Postgres row-level security (RLS) with two tenants, the same login role your application or agent actually uses, and both positive and negative assertions. Prove that each tenant can read and write its own rows, cannot read or write another tenant's rows, and loses its transaction-scoped tenant context after commit.

There is one extra trap in a disposable PGSandbox database: the generated sandbox role owns the database and usually owns the tables it creates. PostgreSQL table owners normally bypass RLS. A test can therefore appear to validate a policy while never running through it. Use `FORCE ROW LEVEL SECURITY`, then assert `row_security_active()` before trusting any result.

This guide packages the checks into an **RLS Isolation Proof Contract**: identity, activation, visibility, mutation, and context. The included Psycopg harness runs the contract against a disposable database and returns compact evidence suitable for an agent PR.

## In this guide

- [Understand the RLS owner bypass](#why-an-rls-test-can-pass-without-testing-rls)
- [Use the RLS Isolation Proof Contract](#the-rls-isolation-proof-contract)
- [Create a deterministic Psycopg harness](#1-create-a-deterministic-rls-test)
- [Run it in disposable Postgres](#2-run-the-proof-with-pgsandbox)
- [Verify policy structure](#3-verify-the-policy-not-only-query-results)
- [Test reads and writes separately](#4-test-visibility-and-mutation-as-separate-boundaries)
- [Prove tenant context resets](#5-prove-transaction-scoped-context-does-not-leak)
- [Know what the proof does not cover](#what-this-rls-test-does-not-prove)
- [Record PR-ready evidence](#pr-ready-rls-proof)

## Why an RLS test can pass without testing RLS

PostgreSQL row-level security adds policy expressions to ordinary table access. A `USING` expression controls which existing rows are visible to a command. A `WITH CHECK` expression controls which proposed rows an `INSERT` or `UPDATE` may create.

The current PostgreSQL [row security documentation](https://www.postgresql.org/docs/current/ddl-rowsecurity.html) identifies two bypass paths that matter to tests:

- Superusers and roles with `BYPASSRLS` always bypass row security.
- Table owners normally bypass row security unless the table uses `FORCE ROW LEVEL SECURITY`.

PGSandbox creates a login role with PostgreSQL's non-elevated defaults and makes it the owner of the disposable database. The [per-sandbox role guide](/blog/per-sandbox-postgres-roles-coding-agents/) explains that the role is not granted `SUPERUSER`, `CREATEDB`, `CREATEROLE`, `REPLICATION`, or `BYPASSRLS`. But tables created through that connection still belong to the sandbox role, so the owner exception remains relevant.

`ENABLE ROW LEVEL SECURITY` alone is therefore insufficient for this harness:

```sql
ALTER TABLE tenant_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_documents FORCE ROW LEVEL SECURITY;
```

The second command makes the owner subject to applicable policies. The harness also queries `row_security_active('tenant_documents')`, which PostgreSQL documents as the context-aware answer to whether RLS applies to the current user and environment.

### Empty results are not isolation proof

A query that returns zero rows has several possible explanations: the fixture may be empty, the tenant context may be missing, ordinary table privileges may have denied access first, or an RLS policy may have filtered the rows.

Use paired assertions instead. Tenant A should see known A rows and no B rows. Tenant B should see known B rows and no A rows. That proves the fixture exists and the policy changes visibility with the active tenant context.

The same principle applies to writes. Reading only tenant A rows does not prove that tenant A cannot insert or update a row labeled as tenant B. PostgreSQL's current [`CREATE POLICY` reference](https://www.postgresql.org/docs/current/sql-createpolicy.html) treats `USING` and `WITH CHECK` as different boundaries, so the test should do the same.

## The RLS Isolation Proof Contract

A reviewable RLS test should answer five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Identity | Is the test using the intended non-bypass role? | `current_user`, `rolsuper = false`, `rolbypassrls = false` |
| Activation | Is RLS enabled and forced for the table owner? | `relrowsecurity`, `relforcerowsecurity`, and `row_security_active()` are true |
| Visibility | Can each tenant read its own fixture and no foreign fixture? | Paired tenant A and tenant B result sets |
| Mutation | Can a tenant write its own row while a cross-tenant write fails? | Successful same-tenant insert and SQLSTATE `42501` for the foreign insert |
| Context | Does request identity end with the transaction? | `SET LOCAL`/`set_config(..., true)` resets after commit; a context-free query sees no rows |

This contract is the information gain over a single `SELECT count(*)` check. It verifies the execution identity, the policy's active state, both halves of the authorization rule, and the lifetime of request context.

## 1. Create a deterministic RLS test

The following Psycopg 3 script uses the sandbox role as both table owner and test identity. It seeds two tenant rows before enabling RLS, forces the owner through the policy, then runs tenant-specific transactions.

Save it as `tests/postgres_rls_proof.py`:

```python
import json
import os
import uuid

import psycopg
from psycopg import errors


DATABASE_URL = os.environ["PGSANDBOX_DATABASE_URL"]
TENANT_A = uuid.UUID("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")
TENANT_B = uuid.UUID("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")


def reset_fixture():
    with psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    ) as conn:
        conn.execute("DROP TABLE IF EXISTS tenant_documents")
        conn.execute(
            """
            CREATE TABLE tenant_documents (
                id integer PRIMARY KEY,
                tenant_id uuid NOT NULL,
                body text NOT NULL
            )
            """
        )
        for row in [
            (1, TENANT_A, "tenant-a-seed"),
            (2, TENANT_B, "tenant-b-seed"),
        ]:
            conn.execute(
                """
                INSERT INTO tenant_documents (id, tenant_id, body)
                VALUES (%s, %s, %s)
                """,
                row,
            )
        conn.execute(
            "ALTER TABLE tenant_documents ENABLE ROW LEVEL SECURITY"
        )
        conn.execute(
            "ALTER TABLE tenant_documents FORCE ROW LEVEL SECURITY"
        )
        conn.execute(
            """
            CREATE POLICY tenant_isolation
            ON tenant_documents
            AS PERMISSIVE
            FOR ALL
            TO PUBLIC
            USING (
                tenant_id =
                current_setting('app.tenant_id', true)::uuid
            )
            WITH CHECK (
                tenant_id =
                current_setting('app.tenant_id', true)::uuid
            )
            """
        )


def set_local_tenant(conn, tenant_id):
    conn.execute(
        "SELECT set_config('app.tenant_id', %s, true)",
        (str(tenant_id),),
    )


def prove_identity_and_activation(conn):
    identity = conn.execute(
        """
        SELECT
            current_user,
            rolsuper,
            rolbypassrls
        FROM pg_roles
        WHERE rolname = current_user
        """
    ).fetchone()
    assert identity is not None
    assert identity[1:] == (False, False), identity

    activation = conn.execute(
        """
        SELECT
            c.relowner = (SELECT oid FROM pg_roles
                          WHERE rolname = current_user),
            c.relrowsecurity,
            c.relforcerowsecurity,
            row_security_active(c.oid)
        FROM pg_class AS c
        WHERE c.oid = 'tenant_documents'::regclass
        """
    ).fetchone()
    assert activation == (True, True, True, True), activation

    policy = conn.execute(
        """
        SELECT permissive, cmd, qual IS NOT NULL, with_check IS NOT NULL
        FROM pg_policies
        WHERE schemaname = current_schema()
          AND tablename = 'tenant_documents'
          AND policyname = 'tenant_isolation'
        """
    ).fetchone()
    assert policy == ("PERMISSIVE", "ALL", True, True), policy

    return {
        "currentUser": identity[0],
        "superuser": identity[1],
        "bypassRls": identity[2],
        "ownsTable": activation[0],
        "rlsEnabled": activation[1],
        "rlsForced": activation[2],
        "rlsActive": activation[3],
        "policy": {
            "name": "tenant_isolation",
            "command": policy[1],
            "hasUsing": policy[2],
            "hasWithCheck": policy[3],
        },
    }


def prove_tenant_a(conn):
    with conn.transaction():
        set_local_tenant(conn, TENANT_A)

        visible = conn.execute(
            "SELECT id FROM tenant_documents ORDER BY id"
        ).fetchall()
        assert visible == [(1,)], visible

        conn.execute(
            """
            INSERT INTO tenant_documents (id, tenant_id, body)
            VALUES (%s, %s, %s)
            """,
            (3, TENANT_A, "tenant-a-own-write"),
        )

        try:
            with conn.transaction():
                conn.execute(
                    """
                    INSERT INTO tenant_documents (id, tenant_id, body)
                    VALUES (%s, %s, %s)
                    """,
                    (4, TENANT_B, "tenant-a-cross-write"),
                )
        except errors.InsufficientPrivilege as exc:
            assert exc.sqlstate == "42501", exc.sqlstate
            denied_sqlstate = exc.sqlstate
        else:
            raise AssertionError("cross-tenant insert unexpectedly succeeded")

        surviving = conn.execute(
            "SELECT id FROM tenant_documents ORDER BY id"
        ).fetchall()
        assert surviving == [(1,), (3,)], surviving

    return {
        "visibleIds": [1],
        "ownWrite": 3,
        "crossTenantWriteSqlstate": denied_sqlstate,
    }


def prove_tenant_b(conn):
    with conn.transaction():
        set_local_tenant(conn, TENANT_B)
        visible = conn.execute(
            "SELECT id FROM tenant_documents ORDER BY id"
        ).fetchall()
        assert visible == [(2,)], visible

    return {"visibleIds": [2]}


def prove_context_reset(conn):
    with conn.transaction():
        configured = conn.execute(
            "SELECT current_setting('app.tenant_id', true)"
        ).fetchone()[0]
        assert configured in (None, ""), repr(configured)

        visible = conn.execute(
            "SELECT id FROM tenant_documents ORDER BY id"
        ).fetchall()
        assert visible == [], visible

    return {"tenantSetting": configured, "visibleIds": []}


def main():
    reset_fixture()

    with psycopg.connect(
        DATABASE_URL,
        autocommit=True,
        connect_timeout=5,
    ) as conn:
        evidence = {
            "identityAndActivation": prove_identity_and_activation(conn),
            "tenantA": prove_tenant_a(conn),
            "tenantB": prove_tenant_b(conn),
            "contextReset": prove_context_reset(conn),
        }

    print(json.dumps(evidence, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
```

The inner `conn.transaction()` around the denied insert becomes a savepoint because it is nested inside Tenant A's outer transaction. Psycopg rolls that savepoint back when PostgreSQL reports `42501`, allowing the test to prove that the accepted same-tenant insert remains part of the outer transaction.

The [savepoint and partial rollback guide](/blog/test-postgres-savepoints-partial-rollbacks/) covers that recovery mechanism in detail. Here, the savepoint is test plumbing; the authorization decision is the result under test.

## 2. Run the proof with PGSandbox

Run the script inside a one-shot disposable database:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --name-hint rls-proof \
  --owner rls-ci \
  --ttl-minutes 30 \
  --cleanup always \
  --timeout-seconds 60 \
  --result-format json \
  -- uv run --with 'psycopg[binary]' \
     python tests/postgres_rls_proof.py
```

The [disposable Postgres integration-test workflow](/blog/run-integration-tests-disposable-postgres-database/) explains the session envelope. PGSandbox creates the database and scoped login, injects `PGSANDBOX_DATABASE_URL`, captures bounded redacted output, and applies the requested cleanup policy.

Use `--cleanup always` in CI. During investigation, `--cleanup on-success` keeps a failed sandbox until its TTL so an operator can inspect the safe database identifier returned by PGSandbox. Do not print or save the injected database URL.

If you use the MCP lifecycle tools instead of the CLI, call `create_database`, obtain the task connection through the configured secret-handling path, run the harness as one process, then call `delete_database`. The [MCP tool contract](/docs/mcp-tools/) keeps lifecycle authority separate from task SQL.

## 3. Verify the policy, not only query results

The harness checks three PostgreSQL metadata surfaces before touching tenant data:

1. `pg_roles` confirms the session is neither a superuser nor a `BYPASSRLS` role.
2. `pg_class` confirms RLS is enabled and forced on the owner.
3. `pg_policies` confirms the named policy has both a visibility expression and a write-check expression.

PostgreSQL documents `relrowsecurity` and `relforcerowsecurity` in [`pg_class`](https://www.postgresql.org/docs/current/catalog-pg-class.html). It exposes policy command, roles, `qual`, and `with_check` through [`pg_policies`](https://www.postgresql.org/docs/current/view-pg-policies.html).

These checks catch structural regressions that result assertions can miss. A test fixture with only Tenant A rows could still return the expected A row if the policy disappeared. A metadata assertion plus a two-tenant fixture closes that gap.

Do not snapshot the exact rendered policy expression unless your migration review needs it. PostgreSQL may normalize casts or parentheses in catalog output. Assert the security properties first: the expected policy exists, applies to the intended command and role set, and contains both boundaries.

## 4. Test visibility and mutation as separate boundaries

`USING` and `WITH CHECK` answer different questions:

| Policy boundary | Applies to | Expected negative behavior |
| --- | --- | --- |
| `USING` | Existing rows considered by `SELECT`, `UPDATE`, and `DELETE` | Foreign rows are filtered from visibility |
| `WITH CHECK` | Proposed rows from `INSERT` and `UPDATE` | The statement fails when the new row violates the policy |

That difference affects test assertions. A cross-tenant `SELECT` normally returns no foreign row; it does not need to raise an error. A cross-tenant `INSERT` should fail because the proposed row violates `WITH CHECK`.

The harness catches Psycopg's `InsufficientPrivilege` class and verifies SQLSTATE `42501`. Psycopg's current [error class reference](https://www.psycopg.org/psycopg3/docs/api/errors.html) maps server SQLSTATEs to typed exceptions. Prefer the stable code over matching the wording of PostgreSQL's error message.

Add `UPDATE` and `DELETE` cases when those commands have separate policies in your application. For a policy defined `FOR ALL`, the compact harness tests the two core directions, but production policy suites should cover every command-specific rule and every applicable role.

## 5. Prove transaction-scoped context does not leak

Multi-tenant applications often store request identity in a custom PostgreSQL setting:

```sql
SELECT set_config('app.tenant_id', '<tenant-uuid>', true);
```

The third argument makes the change local to the current transaction. PostgreSQL's [parameter-setting documentation](https://www.postgresql.org/docs/current/config-setting.html) describes `set_config(name, value, is_local)` as the function equivalent of `SET`; the [`SET` reference](https://www.postgresql.org/docs/current/sql-set.html) says `LOCAL` settings end at commit or rollback.

The harness opens a new transaction after the Tenant A and Tenant B proofs. It confirms the custom value is empty and an RLS-protected query sees no rows. That check matters when a connection pool reuses sessions across requests: tenant identity should be transaction state, not an indefinite session default.

Application code must derive the tenant value from authenticated server-side context. Never trust a raw tenant identifier supplied by the caller merely because the RLS policy reads it.

## What this RLS test does not prove

This harness proves one table, one policy shape, one execution role, and one transaction-context pattern. It does not prove:

- that every tenant-bearing table has RLS enabled;
- that views, functions, triggers, or security-definer code preserve the same boundary;
- that every application path sets tenant context before its first query;
- that foreign keys, unique constraints, and other referential-integrity checks reveal no information across policy boundaries;
- that policy predicates use suitable indexes at production data volume;
- that pool reset behavior is correct after a network failure or process crash.

Expand the contract across your real schema. Query `pg_class` for tenant tables without `relrowsecurity`, enumerate `pg_policies`, run the application test suite through its production-like role, and add plan checks for indexed tenant predicates where performance matters.

PGSandbox is deliberately not a production operations tool. It gives the agent a private place to run destructive and negative authorization tests before a PR; your deployment and production access controls remain separate review gates.

## PR-ready RLS proof

Record evidence without credentials:

```json
{
  "database": "disposable PGSandbox database",
  "role": {
    "superuser": false,
    "bypassRls": false,
    "ownsTable": true
  },
  "activation": {
    "enabled": true,
    "forcedForOwner": true,
    "active": true
  },
  "visibility": {
    "tenantA": [1],
    "tenantB": [2]
  },
  "mutation": {
    "sameTenantInsert": "passed",
    "crossTenantInsertSqlstate": "42501"
  },
  "contextReset": "passed",
  "cleanup": "deleted"
}
```

The pass condition is explicit: the intended non-bypass identity ran through an active forced policy, each tenant saw only its rows, the same-tenant write survived, the cross-tenant write failed with `42501`, transaction-local context disappeared, and PGSandbox deleted the task database.

That is stronger than "the RLS query returned the expected count." It is a compact authorization proof an agent can attach to a migration PR without exposing database credentials or relying on shared development state.
