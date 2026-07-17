---
title: "PostgreSQL ROLE vs USER for Agent Database Access"
excerpt: "PostgreSQL users and roles are the same underlying object. Learn what LOGIN changes, when to use NOLOGIN roles, and how to design a coding-agent credential."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-17"
updatedAt: "2026-07-17T06:00:00Z"
tags: ["Postgres", "database roles", "database users", "MCP", "coding agents"]
category: "Engineering"
metaTitle: "PostgreSQL ROLE vs USER for Agent Access"
metaDescription: "PostgreSQL ROLE vs USER explained: compare LOGIN, membership, ownership, and choose a scoped credential model for coding-agent database work."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/postgres-role-vs-user-agent-access/"
heroImageUrl: ""
featured: false
sortOrder: 135
---
In PostgreSQL, a user and a role are the same kind of database object. `CREATE USER` is alternate syntax for `CREATE ROLE` that enables `LOGIN` by default. For coding-agent database access, the useful question is not "role or user?" It is whether the role should authenticate, inherit privileges, own objects, or combine those jobs.

The short recommendation is:

- Use a fresh `LOGIN` role as the agent's connection identity.
- Make that role the owner of its disposable task database when the agent must run migrations and create objects.
- Use separate `NOLOGIN` roles only when you need reusable privilege bundles across multiple identities.
- Keep database and role creation authority on the lifecycle side, outside the credential used for task SQL.

PGSandbox follows the direct version of this model: one generated login role owns one tracked sandbox database. The agent uses that identity for database work, while the server retains lifecycle authority through its configured admin connection.

## PostgreSQL ROLE vs USER at a glance

PostgreSQL has used one unified role model since version 8.1. The current [database roles documentation](https://www.postgresql.org/docs/current/user-manag.html) says a role can act as a database user, a group of users, or both, depending on its attributes and memberships.

| Question | `CREATE ROLE agent_task` | `CREATE USER agent_task` |
| --- | --- | --- |
| Underlying catalog object | PostgreSQL role | PostgreSQL role |
| Can log in by default? | No (`NOLOGIN`) | Yes (`LOGIN`) |
| Can own objects? | Yes | Yes |
| Can receive privileges or memberships? | Yes | Yes |
| Can act as a group role? | Yes | Technically yes, though the name suggests an identity |
| Best use in agent infrastructure | Explicit login or privilege role, depending on attributes | Shorthand for a login identity |

The only default that changes is login. PostgreSQL's [`CREATE USER` reference](https://www.postgresql.org/docs/current/sql-createuser.html) defines it as an alternative spelling of `CREATE ROLE`, with `LOGIN` assumed unless you override it. [`CREATE ROLE`](https://www.postgresql.org/docs/current/sql-createrole.html) assumes `NOLOGIN` unless you add `LOGIN`.

These commands therefore create equivalent login identities:

```sql
CREATE USER agent_task PASSWORD 'use-a-generated-secret';

CREATE ROLE agent_task LOGIN PASSWORD 'use-a-generated-secret';
```

For infrastructure code, the second form is often clearer because the capability is visible. A reviewer does not need to remember the special default attached to the word `USER`.

## What makes a PostgreSQL role a user?

The `LOGIN` attribute makes a role usable as the initial authorization identity for a database connection. PostgreSQL's [`pg_roles` view](https://www.postgresql.org/docs/current/view-pg-roles.html) exposes this as `rolcanlogin`.

A password alone does not make a role a user. A `NOLOGIN` role may have a password field, but it still cannot be supplied as the initial role name for a client connection. Conversely, a `LOGIN` role may authenticate through a method other than a password, depending on `pg_hba.conf` and the server's authentication configuration.

This distinction gives operators two useful building blocks:

```sql
CREATE ROLE agent_task LOGIN PASSWORD 'generated-secret';
CREATE ROLE migration_writer NOLOGIN;
```

`agent_task` is an authentication identity. `migration_writer` is a privilege carrier. You can grant the latter to one or more login roles when a shared capability model is actually needed:

```sql
GRANT migration_writer TO agent_task;
```

PostgreSQL's [role membership documentation](https://www.postgresql.org/docs/current/role-membership.html) explains two ways a member can use those privileges. A membership with `INHERIT` makes ordinary object privileges available automatically. A membership with `SET` allows the session to run `SET ROLE migration_writer` and use the target role's privileges and ownership context.

That flexibility is useful in long-lived application environments. It is often unnecessary in a disposable agent sandbox, where a single generated role can be both the login identity and the database owner for the duration of one task.

## Use three axes instead of the user-versus-role label

The `ROLE` versus `USER` vocabulary hides the decisions that determine real authority. Classify every credential on three independent axes.

### 1. Authentication identity

Can a client start a session as this role? If yes, it needs `LOGIN` and an authentication path accepted by the server. An agent connection string must name a login role.

### 2. Privilege carrier

Does the role collect privileges that other roles inherit or assume? A shared read, write, migration, or reporting capability normally fits a `NOLOGIN` role. Membership edges then become part of the access-control review.

### 3. Object owner

Does the role own the database, schema, table, or function? PostgreSQL's [privilege documentation](https://www.postgresql.org/docs/current/ddl-priv.html) notes that ownership carries inherent authority to alter or drop the object. Ownership is stronger than an ordinary grant and should follow the lifecycle of the work.

These axes can overlap. A role may be a login identity, a member of capability roles, and an owner at the same time. Treating "user" as one kind of object and "role" as another makes audits harder because PostgreSQL itself does not enforce that split.

For agent database work, use this decision table:

| Role design | Login | Shared membership | Owns task database | When it fits |
| --- | --- | --- | --- | --- |
| Disposable sandbox identity | Yes | Usually none | Yes | One agent task needs full migration and test authority inside disposable state |
| Read-only inspection identity | Yes | Optional read role | No | An agent may inspect approved shared state but must not mutate it |
| Shared capability role | No | Granted to login roles | Maybe owns selected objects | Several stable identities need the same reviewed privileges |
| Lifecycle admin | Yes or local operator path | Broad by necessity | Creates and deletes resources | Server-side lifecycle only; never the routine task credential |

The table is the practical difference between syntax and design. `CREATE USER` can create the first, second, or fourth identity. `CREATE ROLE` can create all four. The attributes, memberships, ownership, and tool boundary decide the risk.

## How PGSandbox models an agent database identity

PGSandbox creates a role explicitly with `LOGIN`, then creates a database owned by that role:

```sql
CREATE ROLE "<generated_role>" LOGIN PASSWORD '<generated_secret>';
CREATE DATABASE "<generated_database>" OWNER "<generated_role>";
```

The current source does not grant the generated identity membership in a reusable group role. PostgreSQL's defaults also leave it without `SUPERUSER`, `CREATEDB`, `CREATEROLE`, `REPLICATION`, or `BYPASSRLS`. The result is deliberately simple: one authentication identity, one database owner, and one task lifecycle.

The [PGSandbox architecture](/docs/architecture/) separates that role from the admin connection used to create and delete databases and roles. The [`create_database` tool contract](/docs/mcp-tools/) returns the generated role name and redacted connection information, while credential-bearing connection strings are returned only on an explicit request and should not be copied into logs or PR comments.

This design also matches MCP's [scope-minimization guidance](https://modelcontextprotocol.io/docs/tutorials/security/security_best_practices): start with the narrow authority required for the operation instead of handing every caller a catch-all credential. For PGSandbox, lifecycle tools hold lifecycle authority and sandbox-scoped tools use the task role.

The generated role is still cluster-wide because all PostgreSQL roles are cluster-wide. Database ownership does not prove that the identity cannot connect elsewhere. The [per-sandbox Postgres role guide](/blog/per-sandbox-postgres-roles-coding-agents/) covers `CONNECT`, `pg_hba.conf`, memberships, and cross-database policy without pretending a username creates a cluster boundary.

## How to inspect users and roles without confusing the views

Use `pg_roles` for access reviews because it includes every role and exposes `rolcanlogin`:

```sql
SELECT rolname,
       rolcanlogin,
       rolsuper,
       rolcreatedb,
       rolcreaterole,
       rolreplication,
       rolbypassrls
FROM pg_roles
ORDER BY rolname;
```

PostgreSQL also provides [`pg_user`](https://www.postgresql.org/docs/current/view-pg-user.html), a user-oriented view with fields such as `usename`, `usecreatedb`, and `usesuper`. It is useful for compatibility and familiar reporting, but it does not replace `pg_roles` when you need to see both login and non-login roles.

Inside a session, inspect the authentication and effective identities together:

```sql
SELECT session_user, current_user, current_role, current_database();
```

The [system information reference](https://www.postgresql.org/docs/current/functions-info.html) defines `current_role` as equivalent to `current_user`. `session_user` identifies the session user, while `current_user` can change after `SET ROLE` or inside a security-definer context. If an agent workflow permits role switching, recording both values makes the proof more useful than recording a generic "database user" label.

For the standard PGSandbox path, `session_user` and `current_user` should both be the generated sandbox role. An unexpected difference is a reason to inspect memberships and the SQL executed earlier in the session.

## Common PostgreSQL ROLE vs USER mistakes

### Choosing `CREATE USER` because it sounds more restricted

It is not more restricted. It creates a role with `LOGIN` by default. Add the exact negative attributes you care about or verify the PostgreSQL defaults instead of relying on the noun.

### Giving every capability role `LOGIN`

A shared capability role normally does not need a credential or direct authentication path. Keep it `NOLOGIN`, grant it deliberately, and audit `INHERIT` and `SET` behavior.

### Assuming membership attributes are ordinary privileges

PostgreSQL documents `LOGIN`, `SUPERUSER`, `CREATEDB`, and `CREATEROLE` as special role attributes. They are not inherited like table privileges. A membership design must account for whether the session can `SET ROLE` to a role carrying those attributes.

### Separating login and ownership without a reason

In a long-lived application, separate login roles and owner roles can reduce routine authority. In a disposable task database, that extra membership graph may add complexity without improving the boundary. A fresh login role that owns only the disposable database is easier to create, verify, and delete as one unit.

### Passing the lifecycle admin identity to the agent

No naming convention repairs an overpowered connection. The [Postgres MCP safety checklist](/blog/postgres-mcp-server-safety-checklist/) keeps admin credentials server-side and gives task SQL a narrower path. Use the lifecycle API to create the sandbox; use the generated login role to work inside it.

## A credential policy you can copy

```text
Treat every PostgreSQL user as a role with attributes. Agent task credentials
must be explicit LOGIN roles with no SUPERUSER, CREATEDB, CREATEROLE,
REPLICATION, or BYPASSRLS. A writable agent role may own only its disposable
task database. Reusable privilege bundles must be separate NOLOGIN roles, with
every membership reviewed for INHERIT and SET behavior. Keep the lifecycle
admin credential outside task SQL and remove the task database and login role
together when the proof is complete.
```

If the workflow also needs execution safeguards, combine this identity policy with [bounded Postgres result handling](/blog/postgres-run-sql-bounded-results/). Role design controls authority; readonly mode, row limits, timeouts, and cleanup control how that authority is exercised.

## Frequently asked questions

### Are PostgreSQL roles and users the same?

Yes. PostgreSQL has one role object that can act as a user, a group, or both. A role with `LOGIN` can be the initial identity for a database connection and is what PostgreSQL commonly calls a user. `CREATE USER` is shorthand for creating a role with `LOGIN` enabled by default.

### Should an application use CREATE USER or CREATE ROLE?

Either command can create the same login identity. `CREATE ROLE app_name LOGIN` is more explicit in infrastructure code because the login capability appears in the statement. `CREATE USER app_name` is valid shorthand. Security comes from attributes, memberships, ownership, authentication policy, and grants, not the command spelling.

### Can a role without LOGIN own PostgreSQL objects?

Yes. `NOLOGIN` prevents the role from being used as the initial connection identity; it does not prevent ownership or privileges. Long-lived applications often use a `NOLOGIN` owner role and grant controlled membership to login roles. Disposable agent databases can use one short-lived login role as owner when that simpler lifecycle matches the boundary.

### Is a PostgreSQL role limited to one database?

No. Roles exist across the PostgreSQL cluster. A role may own one database by policy, but authentication rules, database `CONNECT`, memberships, schema privileges, and object grants determine what it can reach. Use a dedicated cluster or explicit connection policy when database-level ownership is not a sufficient boundary.

### Does PGSandbox create a user or a role?

PGSandbox runs `CREATE ROLE ... LOGIN`, so PostgreSQL treats the generated identity as both a role and a database user. The role owns the disposable sandbox database and is used for task SQL. PGSandbox separately retains the admin connection for tracked lifecycle operations.

## Related pages

- [PGSandbox architecture](/docs/architecture/)
- [PGSandbox MCP tool contract](/docs/mcp-tools/)
- [Per-sandbox Postgres roles for coding agents](/blog/per-sandbox-postgres-roles-coding-agents/)
- [Postgres MCP server safety checklist](/blog/postgres-mcp-server-safety-checklist/)
- [How to run agent SQL with bounded results](/blog/postgres-run-sql-bounded-results/)

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Article",
      "headline": "PostgreSQL ROLE vs USER for Agent Database Access",
      "datePublished": "2026-07-17",
      "dateModified": "2026-07-17",
      "author": {"@type": "Organization", "name": "PGSandbox Team"},
      "mainEntityOfPage": "https://pgsandbox-mcp.lvtd.dev/blog/postgres-role-vs-user-agent-access/"
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "PostgreSQL ROLE vs USER for Agent Database Access", "item": "https://pgsandbox-mcp.lvtd.dev/blog/postgres-role-vs-user-agent-access/"}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "Are PostgreSQL roles and users the same?", "acceptedAnswer": {"@type": "Answer", "text": "Yes. PostgreSQL has one role object. A role with LOGIN can be the initial identity for a database connection and is commonly called a user. CREATE USER is shorthand for creating a role with LOGIN enabled by default."}},
        {"@type": "Question", "name": "Should an application use CREATE USER or CREATE ROLE?", "acceptedAnswer": {"@type": "Answer", "text": "Either command can create the same login identity. CREATE ROLE with an explicit LOGIN attribute makes the capability visible in infrastructure code; CREATE USER is valid shorthand."}},
        {"@type": "Question", "name": "Can a role without LOGIN own PostgreSQL objects?", "acceptedAnswer": {"@type": "Answer", "text": "Yes. NOLOGIN prevents a role from being used as the initial connection identity. It does not prevent that role from owning objects, receiving privileges, or being granted to other roles."}},
        {"@type": "Question", "name": "Is a PostgreSQL role limited to one database?", "acceptedAnswer": {"@type": "Answer", "text": "No. Roles exist across a PostgreSQL cluster. Authentication rules, database CONNECT, memberships, schema privileges, and object grants determine what a role can reach."}},
        {"@type": "Question", "name": "Does PGSandbox create a user or a role?", "acceptedAnswer": {"@type": "Answer", "text": "PGSandbox runs CREATE ROLE with LOGIN, so the generated identity is both a PostgreSQL role and a database user. It owns the disposable sandbox database and is used for task SQL."}}
      ]
    }
  ]
}
</script>
