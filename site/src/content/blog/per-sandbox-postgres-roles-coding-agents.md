---
title: "Per-Sandbox Postgres Roles for Coding Agents"
excerpt: "Use one login role per disposable Postgres database, keep admin credentials out of task SQL, and understand exactly what the role boundary does and does not isolate."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-16"
updatedAt: "2026-07-16T06:00:00Z"
tags: ["Postgres", "MCP", "database roles", "least privilege", "coding agents"]
category: "Engineering"
metaTitle: "Per-Sandbox Postgres Roles for Coding Agents"
metaDescription: "Use one Postgres role per coding-agent sandbox, separate lifecycle authority from task SQL, verify privileges, and clean up the role with the database."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/per-sandbox-postgres-roles-coding-agents/"
heroImageUrl: ""
featured: false
sortOrder: 134
---
A per-sandbox Postgres role gives each coding-agent task its own login identity and database owner. The lifecycle service uses an admin connection to create and delete resources; the agent runs migrations, seeds, and validation SQL through the generated role. This removes broad lifecycle authority from the normal task path and makes the database and credential one disposable unit.

PostgreSQL does not store users and roles as separate object types. If the terminology is the confusing part, start with [PostgreSQL ROLE vs USER for agent access](/blog/postgres-role-vs-user-agent-access/): it explains the `LOGIN` default, `NOLOGIN` capability roles, memberships, and ownership before this guide applies them to a per-sandbox boundary.

The important caveat is that PostgreSQL roles are cluster-wide, not database-local. A role created for one sandbox is valid throughout the cluster. Database ownership and object privileges limit what it can do, while `CONNECT`, `pg_hba.conf`, role memberships, and default grants determine where it can authenticate and what else it can reach. "One role per database" is a useful authority boundary, but it is not a network or cluster boundary by itself.

This guide turns that distinction into a practical operating model:

> **task authority = unique login role + owned sandbox database + no elevated attributes + bounded tool path + paired cleanup**

That model is stricter than giving an agent the Postgres admin URL, and more honest than claiming a generated username alone creates perfect isolation.

## Quick checklist for a role per sandbox database

For each coding-agent task:

1. Create a fresh `LOGIN` role with a generated secret and no superuser, database-creation, role-creation, replication, or row-security-bypass attributes.
2. Create one disposable database owned by that role.
3. Give the agent only the connection string for that role and database, never the admin profile URL.
4. Run task SQL through a narrow tool such as `run_sql`, with readonly mode and result limits where the proof does not require writes.
5. Verify the session identity, current database, role attributes, database owner, and unexpected memberships before trusting the sandbox.
6. Delete the database first, then drop its role, and mark the tracked resource deleted only after both operations succeed.

PGSandbox implements the core pair automatically. Its [`create_database` tool](/docs/mcp-tools/) creates a generated login role, creates a database owned by that role, records both in lifecycle metadata, and returns a redacted task connection. Its [architecture](/docs/architecture/) keeps the admin connection in the lifecycle layer and uses the sandbox role for user SQL.

## Why give every coding-agent task a separate Postgres role?

A separate role makes database authority disposable at the same scope as the task. If three agent tasks share one application login, PostgreSQL sees the same identity for all three. Grants, object ownership, active sessions, and cleanup dependencies become shared. A mistake in one task can affect state created by another task because the database cannot distinguish their authority.

One role per sandbox changes the unit of control:

| Question | Shared application role | Per-sandbox role |
| --- | --- | --- |
| Who owns task-created objects? | A long-lived shared identity | The task's generated identity |
| Which credential can be revoked after the task? | Revocation disrupts other work | The task credential can disappear with the sandbox |
| Can task SQL create another database? | Depends on shared role attributes | No, unless an operator deliberately adds `CREATEDB` |
| Can task SQL create more roles? | Depends on shared role attributes | No, unless an operator deliberately adds `CREATEROLE` |
| Can cleanup pair database and login? | Requires convention or external mapping | Yes, lifecycle metadata records both |
| Does the role prove cluster-level confinement? | No | Still no; roles exist at cluster scope |

The value is not that the agent becomes harmless. A database owner can still change or drop objects inside its database. That is the point of a writable task sandbox: the agent needs enough authority to prove migrations and application behavior. The safety improvement is that this destructive authority lands in a disposable database instead of a shared development or production database.

This is also why a sandbox role should not be a generic read-only reporting role. Agent development work often needs `CREATE TABLE`, `ALTER TABLE`, data writes, extension installation, and rollback testing. The right target is **full task authority inside a disposable boundary**, not a weak credential that cannot exercise the code under review.

## What PGSandbox creates for one task

The current PGSandbox implementation builds the resource pair in a short, auditable sequence:

```sql
CREATE ROLE "<generated_role>" LOGIN PASSWORD '<generated_secret>';
CREATE DATABASE "<generated_database>" OWNER "<generated_role>";
```

The generated names share a base plus a random identifier, and the role ends in `_role`. PostgreSQL limits identifiers to 63 bytes; PGSandbox trims generated names to fit that boundary. The role secret is generated separately, encrypted before it is persisted in PGSandbox metadata, and only exposed through the task connection-string flow.

The implementation does not add elevated attributes to `CREATE ROLE`. PostgreSQL documents `NOSUPERUSER`, `NOCREATEDB`, `NOCREATEROLE`, `NOREPLICATION`, and `NOBYPASSRLS` as the defaults when their elevated counterparts are omitted ([PostgreSQL `CREATE ROLE`](https://www.postgresql.org/docs/current/sql-createrole.html)). The role can log in and own its database, but it cannot create another database, create another role, replicate, bypass row-level security, or bypass ordinary permission checks.

PGSandbox then builds the task connection string by replacing the admin URL's username, password, and database path with the generated role, generated secret, and generated database. Calls such as `run_sql`, schema inspection, and requested extension installation use that sandbox connection. The admin connection remains responsible for lifecycle work that the task role should not perform.

This separation follows the least-privilege direction in the Model Context Protocol's [security best practices](https://modelcontextprotocol.io/docs/tutorials/security/security_best_practices): narrow the available scopes and grant more authority only when it is required. For a database MCP server, the equivalent is keeping `CREATEDB` and role-management authority behind lifecycle tools instead of exposing an unrestricted admin SQL credential.

## What does the role boundary actually isolate?

The role boundary isolates **authority and ownership inside PostgreSQL**, but only according to PostgreSQL's complete permission model.

### It separates lifecycle SQL from task SQL

PostgreSQL requires a superuser or a role with `CREATEDB` to create a database. Creating a database owned by another role also requires suitable authority to act as that role ([PostgreSQL `CREATE DATABASE`](https://www.postgresql.org/docs/current/sql-createdatabase.html)). Those operations belong in the server's lifecycle path.

The generated task role does not receive `CREATEDB`. If agent-generated SQL tries to create another database, PostgreSQL rejects it. The agent can modify its owned database without inheriting the authority that created the sandbox fleet.

### It gives task-created objects a disposable owner

PostgreSQL object owners have inherent authority to alter or drop their objects. Ownership is not an ordinary privilege that can be granted and revoked independently ([PostgreSQL privileges](https://www.postgresql.org/docs/current/ddl-priv.html)). Making the task role the database owner aligns that power with the task's disposable resource.

This alignment also makes clone and template restores easier to reason about. PGSandbox uses `pg_dump` and `pg_restore` without restoring source owners or privileges, then restores through the sandbox role. Source-environment identities do not need to become valid identities in the destination. The [safe Postgres clone workflow](/blog/how-to-clone-postgres-database-sandbox/) covers that path in detail.

### It creates a revocable task credential

The credential is useful only while its role exists. PGSandbox cleanup terminates connections, drops the database, drops the matching role, and then records the resource as deleted. A task that finishes cleanly no longer needs a live database login.

Pairing the role and database matters. Dropping only the database leaves a cluster-wide login behind. Dropping only the role usually fails while it still owns the database or objects. The [stale sandbox cleanup guide](/blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/) explains how metadata-backed cleanup selects the pair without guessing from a name prefix.

## What does a per-sandbox role not isolate?

The generated role is not a container, network namespace, separate PostgreSQL cluster, or proof that no other database can be reached.

### PostgreSQL roles are cluster-wide

PostgreSQL's `CREATE ROLE` reference is explicit: roles are defined at the database-cluster level and are valid in every database in that cluster. Calling a role "per-sandbox" describes its intended lifecycle and ownership, not its catalog scope.

That distinction affects cleanup and access reviews. A query against `pg_roles` sees the generated identity at cluster scope. A role may also accumulate memberships or grants outside its intended database if an operator, extension, migration, or manual command adds them. Do not infer current permissions from the role name.

### The database name in a URL is not an access-control list

A connection string selects an initial database. It does not cryptographically bind the role to that database. To connect elsewhere, PostgreSQL checks host-based authentication and the target database's `CONNECT` privilege. PostgreSQL's authentication documentation recommends database `CONNECT` grants and revokes when operators need to restrict which users can connect to which databases ([PostgreSQL `pg_hba.conf`](https://www.postgresql.org/docs/current/auth-pg-hba-conf.html)).

On many clusters, `PUBLIC` has `CONNECT` to databases by default ([PostgreSQL privileges](https://www.postgresql.org/docs/current/ddl-priv.html)). A fresh sandbox role may therefore authenticate to another database if the client knows its name and the host rules allow it. That does not automatically grant access to application tables: schema and object privileges still apply. But it means "the URL points at the sandbox" is not the same claim as "the role cannot connect anywhere else."

PGSandbox's current create path does not rewrite cluster-wide `CONNECT` grants or `pg_hba.conf`. This is deliberate: changing `PUBLIC` access or host authentication on a bring-your-own Postgres cluster would be a much broader operation than creating a disposable task database.

### Database ownership is intentionally strong

The sandbox role owns the database. It can create and alter objects needed for migrations, tests, and bug reproductions. A malicious or incorrect query can still destroy the sandbox's contents, fill its disk allocation, hold locks, or consume connections within the host's remaining limits.

Use the [bounded `run_sql` workflow](/blog/postgres-run-sql-bounded-results/) when the proof needs SQL execution: readonly transactions for inspection, small `rowLimit` values for results, and explicit cleanup afterward. A scoped role limits authority; query controls limit execution behavior and output size. Neither replaces the other.

### It does not make production data safe to copy

Separate credentials do not anonymize data. If a source clone contains customer records or secrets, the sandbox role can read whatever was restored into its owned database. Use sanitized fixtures or an approved data-handling process. PGSandbox is local-first lifecycle tooling, not a data-masking service or a production access broker.

## How to verify a sandbox role before agent work

Treat the returned role as a claim that can be checked. Run a small verification packet before a high-risk migration or when a shared Postgres profile has been manually modified.

### 1. Verify session identity and database

```sql
SELECT current_user, session_user, current_database();
```

`current_user` and `session_user` should be the generated sandbox role in the ordinary PGSandbox task path, and `current_database()` should match the tracked sandbox database.

### 2. Verify elevated role attributes are absent

```sql
SELECT rolname,
       rolcanlogin,
       rolsuper,
       rolcreatedb,
       rolcreaterole,
       rolreplication,
       rolbypassrls,
       rolconnlimit
FROM pg_roles
WHERE rolname = current_user;
```

Expected values are `rolcanlogin = true` and the five elevated booleans set to false. `rolconnlimit` is `-1` unless an operator adds a role-specific connection limit. PostgreSQL documents these fields in the [`pg_roles` view](https://www.postgresql.org/docs/current/view-pg-roles.html).

### 3. Verify database ownership

```sql
SELECT datname,
       pg_get_userbyid(datdba) AS database_owner,
       has_database_privilege(current_user, datname, 'CONNECT') AS can_connect
FROM pg_database
WHERE datname = current_database();
```

The database owner should equal the current sandbox role. PostgreSQL stores database ownership in `pg_database.datdba` ([PostgreSQL `pg_database`](https://www.postgresql.org/docs/current/catalog-pg-database.html)).

### 4. Inspect unexpected memberships

```sql
SELECT granted.rolname AS granted_role
FROM pg_auth_members membership
JOIN pg_roles member ON member.oid = membership.member
JOIN pg_roles granted ON granted.oid = membership.roleid
WHERE member.rolname = current_user;
```

PGSandbox does not grant memberships during ordinary role creation, so an empty result is the expected baseline. Review any membership before using the role. Predefined roles such as `pg_read_all_data`, `pg_write_all_data`, or server-file roles widen authority and should not appear accidentally; PostgreSQL warns that some predefined roles expose privileged information or server-level capabilities ([PostgreSQL predefined roles](https://www.postgresql.org/docs/current/predefined-roles.html)).

### 5. Check cross-database policy separately

If the Postgres host requires strict database-to-role connection mapping, audit `CONNECT` privileges and `pg_hba.conf` as an operator task. Do not ask the coding agent to revoke `PUBLIC` privileges across a shared cluster. A cluster policy change can affect unrelated databases and applications.

For a dedicated PGSandbox cluster, an operator may choose a default-deny connection posture and grant each task role access only to its owned database. For a mixed-use external profile, a separate cluster or host is often easier to reason about than retrofitting per-database host rules around existing applications.

## A practical authority matrix for agent sandboxes

Use this matrix when reviewing a profile or MCP server design:

| Capability | Lifecycle admin | Sandbox role | Coding agent through normal tools |
| --- | --- | --- | --- |
| Create login role | Yes | No | No direct tool |
| Create sandbox database | Yes | No | Only through `create_database` |
| Run migration SQL in sandbox | Avoid | Yes | Through sandbox-scoped tool |
| Create objects in sandbox | Avoid | Yes | Yes when the task requires writes |
| Read bounded proof results | Avoid | Yes | Through `run_sql` result envelope |
| Read another database | Depends on cluster policy | Must be reviewed separately | Should not be part of task workflow |
| Drop sandbox database and role | Yes | No | Only through tracked deletion/cleanup |
| Change `pg_hba.conf` or global grants | Operator concern | No | No |

The matrix is the article's main information-gain framework: **lifecycle authority, task authority, and cluster access are three different scopes**. A safe design states who owns each scope instead of treating "least privilege" as one vague setting.

## Common mistakes with per-sandbox Postgres roles

### Reusing one role across every agent task

This collapses identity, ownership, and revocation back into a shared lane. Give each sandbox its own generated role even when the same agent or repository owns several tasks. Use metadata `owner` and labels to group the tasks operationally; do not reuse the database login for grouping.

### Giving the sandbox role `CREATEDB` or `CREATEROLE`

The agent does not need lifecycle privileges to prove application SQL. Keep those attributes on the admin side. If a migration genuinely creates databases or roles, test it on a dedicated profile with deliberate operator review rather than silently widening every task role.

### Calling the role database-local

It is task-local by policy, not database-local in the PostgreSQL catalog. Document the cluster-level reality so access reviews include `CONNECT`, `pg_hba.conf`, memberships, and default privileges.

### Dropping the database but keeping the login

That leaves an unused credential and cluster catalog entry. PGSandbox pairs deletion of the database and role. If cleanup fails between steps, preserve the failure details and retry through the tracked resource instead of running a broad role-name sweep.

### Passing the admin URL to the agent "temporarily"

Temporary credentials tend to leak into shell history, environment output, logs, or retry context. The [Postgres MCP safety checklist](/blog/postgres-mcp-server-safety-checklist/) treats admin credentials as server-side configuration and recommends returning redacted connection data by default.

### Treating role isolation as data sanitization

The role governs access to the restored data; it does not change that data. Keep sensitive source datasets out of ordinary agent sandboxes.

## Operating policy you can copy

Use this policy in an agent instruction file or internal runbook:

```text
For every database-writing task, create a fresh tracked sandbox and use only its
generated connection. Never expose the PGSandbox admin profile URL to task SQL.
Before high-risk work, verify current_user, current_database(), role attributes,
database ownership, and unexpected memberships. Treat the role as cluster-wide:
the selected database in the URL is not proof of exclusive CONNECT access.
Delete the tracked sandbox after proof is captured so its database and login role
are removed together. Use owner/label-scoped cleanup for interrupted work.
```

Combine this with stable lifecycle ownership and a small [per-owner sandbox quota](/blog/postgres-sandbox-quotas-coding-agents/). The generated login identifies one database resource; the metadata owner identifies the agent or workflow responsible for that resource. They solve different problems and should not use the same naming strategy.

## Frequently asked questions

### Is a PostgreSQL role limited to one database?

No. PostgreSQL roles are created at the cluster level and are valid across databases in that cluster. Database `CONNECT` privileges, host-based authentication, schema privileges, object grants, and role memberships determine what the role can access. A per-sandbox role is a lifecycle and ownership pattern, not an automatic cluster-wide deny rule.

### Why make the sandbox role the database owner?

Migration and test workflows need authority to create, alter, and drop objects. Ownership gives the task role that authority inside its disposable database without granting `CREATEDB`, `CREATEROLE`, or superuser status. Deleting the database afterward removes the task-created object graph as one unit.

### Does PGSandbox give the agent the Postgres admin password?

No. PGSandbox uses the configured admin connection for lifecycle operations and generates a separate role password for each sandbox. Task SQL connects as the generated sandbox role. Returned log-safe connection strings are redacted, while the full task connection is obtained through the dedicated connection flow.

### Should every agent task get a different role?

Yes, when tasks receive independent writable databases. A unique role makes ownership, credential revocation, and cleanup match the task resource. Use stable `owner` metadata to group several task sandboxes for quotas and cleanup; do not share their login credential.

### Does a per-sandbox role prevent destructive SQL?

No. The role owns the sandbox and can run destructive SQL there. The safety property is that the damage is directed at disposable task state. Use readonly mode when writes are unnecessary, bounded outputs for inspection, and a separate database or cluster from production.

### How should the role be removed?

Terminate its sessions, drop the owned sandbox database, drop the matching role, and then mark the tracked resource deleted. PGSandbox's `delete_database` and `cleanup_expired` paths perform this paired lifecycle operation against metadata-owned resources.

## Related pages

- [PGSandbox architecture](/docs/architecture/)
- [PGSandbox MCP tool contract](/docs/mcp-tools/)
- [Postgres MCP server safety checklist](/blog/postgres-mcp-server-safety-checklist/)
- [How to run agent SQL with bounded results](/blog/postgres-run-sql-bounded-results/)
- [Postgres sandbox quotas for coding agents](/blog/postgres-sandbox-quotas-coding-agents/)

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Article",
      "headline": "Per-Sandbox Postgres Roles for Coding Agents",
      "datePublished": "2026-07-16",
      "dateModified": "2026-07-16",
      "author": {"@type": "Organization", "name": "PGSandbox Team"},
      "mainEntityOfPage": "https://pgsandbox-mcp.lvtd.dev/blog/per-sandbox-postgres-roles-coding-agents/"
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "Per-Sandbox Postgres Roles for Coding Agents", "item": "https://pgsandbox-mcp.lvtd.dev/blog/per-sandbox-postgres-roles-coding-agents/"}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "Is a PostgreSQL role limited to one database?", "acceptedAnswer": {"@type": "Answer", "text": "No. PostgreSQL roles exist at cluster scope. CONNECT privileges, host authentication, schema privileges, object grants, and memberships determine what each role can access."}},
        {"@type": "Question", "name": "Why make the sandbox role the database owner?", "acceptedAnswer": {"@type": "Answer", "text": "Database ownership lets the task role create, alter, and drop objects inside its disposable database without receiving CREATEDB, CREATEROLE, or superuser status."}},
        {"@type": "Question", "name": "Does PGSandbox give the agent the Postgres admin password?", "acceptedAnswer": {"@type": "Answer", "text": "No. The admin connection performs lifecycle operations. PGSandbox generates a separate login role and secret for each sandbox, and task SQL uses that role."}},
        {"@type": "Question", "name": "Should every agent task get a different role?", "acceptedAnswer": {"@type": "Answer", "text": "Yes, when tasks receive independent writable databases. A unique role aligns object ownership, credential revocation, and cleanup with one task resource."}},
        {"@type": "Question", "name": "Does a per-sandbox role prevent destructive SQL?", "acceptedAnswer": {"@type": "Answer", "text": "No. The role owns its sandbox and can run destructive SQL there. The safety boundary directs that authority into disposable task state rather than a shared database."}},
        {"@type": "Question", "name": "How should the role be removed?", "acceptedAnswer": {"@type": "Answer", "text": "Terminate its sessions, drop the owned sandbox database, drop the matching role, and mark the tracked resource deleted only after the paired cleanup succeeds."}}
      ]
    }
  ]
}
</script>
