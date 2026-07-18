---
title: "Postgres MCP Server Safety Checklist for Coding Agents"
excerpt: "A practical checklist for giving AI coding agents Postgres access through MCP without handing them shared databases, broad credentials, or unbounded SQL tools."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-06-29"
updatedAt: "2026-06-29T15:34:00Z"
tags: ["postgres", "mcp", "ai-agents", "database-security", "developer-tools"]
category: "Engineering"
metaTitle: "Postgres MCP Server Safety Checklist"
metaDescription: "Use this Postgres MCP server checklist before giving AI coding agents database access: scope credentials, isolate task databases, cap tools, and test cleanup."
canonicalUrl: ""
heroImageUrl: ""
featured: false
sortOrder: 20
---
A Postgres MCP server is safe enough for coding agents only when three things are true: the MCP server exposes a narrow tool surface, the Postgres credentials have a small blast radius, and every task database has a cleanup path. If any one of those is missing, the agent may still be useful, but the database access is running on trust instead of control.

Model Context Protocol (MCP) gives AI applications a standard way to connect to external systems, including databases and tools. The official MCP docs describe it as a way for AI apps like Claude or ChatGPT to connect to data sources, tools, and workflows (https://modelcontextprotocol.io/docs/getting-started/intro). That is exactly why Postgres MCP servers are useful: they let an agent inspect schema, run SQL, validate migrations, or debug backend behavior against real Postgres instead of guessing from code.

The same capability is also the risk. The MCP specification says servers can expose tools as functions for the model to execute, and its trust-and-safety section explicitly warns that tools can represent arbitrary code execution paths that require user consent and caution (https://modelcontextprotocol.io/specification/2025-06-18). A database tool is not just context. It is a path into state.

Use this checklist before giving a coding agent a Postgres MCP server.

## The short checklist

Before an agent touches Postgres through MCP, confirm these controls:

1. The MCP server is local, trusted, version-pinned, and installed from a source you recognize.
2. The tool list is narrow enough to explain in one screen.
3. Read-only tools are actually enforced below the model layer, not just described as read-only.
4. The agent does not receive production or shared development credentials for routine tasks.
5. The admin credential, if one is needed, is used only for lifecycle operations.
6. The task SQL runs as a scoped Postgres role, not as the admin role.
7. The task database is disposable, named, tracked, and owned by the workflow.
8. Query results are bounded so the agent cannot dump large tables by accident.
9. Destructive operations are scoped to resources the server created.
10. Expired resources have a cleanup path that can be audited.

That is the whole safety model in plain English: trusted server, scoped database authority, disposable task state.

## 1. Treat a Postgres MCP server as an execution surface

The first mistake is treating a Postgres MCP server like documentation. It is closer to a command surface.

MCP has passive resources and active tools. The specification lists tools as functions the AI model can execute (https://modelcontextprotocol.io/specification/2025-06-18). A Postgres MCP server that exposes `run_sql`, `create_database`, or `delete_database` is therefore giving the agent an operational handle, not merely a reference page.

That does not make it bad. It means the review should look more like reviewing a CLI or internal admin endpoint. If you want a concrete example of a deliberately narrow tool surface, the [PGSandbox tool contract](https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/) is intentionally small enough to audit before you hand it to an agent.

- Where did this server come from?
- What exact tools does it expose?
- Which tools read data?
- Which tools change data?
- Which tools can create or delete resources?
- Which credentials are loaded into the server process?
- Can the server reach only local/private Postgres, or can it reach production?

Do not rely on the tool description alone. The MCP community has been explicit that annotations such as `readOnlyHint`, `destructiveHint`, `idempotentHint`, and `openWorldHint` are hints, not contracts (https://blog.modelcontextprotocol.io/posts/2026-03-16-tool-annotations/). They help clients present risk, but they do not prove the implementation does what the annotation says.

For Postgres, the enforcement must live in the server code and the database privileges.

## 2. Separate lifecycle authority from task SQL

PostgreSQL users are roles with `LOGIN`, not a separate identity type. The [PostgreSQL ROLE vs USER guide](/blog/postgres-role-vs-user-agent-access/) shows how login identity, privilege membership, and object ownership should be reviewed separately for agent credentials.

A safe Postgres MCP server should not use one powerful connection for everything.

There are two jobs:

- Lifecycle work: create a database, create a role, attach metadata, delete expired resources.
- Task work: apply migrations, seed data, run validation queries, inspect schema.

Those jobs need different authority. PostgreSQL requires superuser or `CREATEDB` privilege to create a database (https://www.postgresql.org/docs/current/sql-createdatabase.html). PostgreSQL roles also control ownership and privileges; the `CREATE ROLE` documentation warns that superuser status bypasses access restrictions and should be used only when needed (https://www.postgresql.org/docs/current/sql-createrole.html).

Use a separate generated login for every writable task database. The [per-sandbox Postgres role guide](/blog/per-sandbox-postgres-roles-coding-agents/) shows the exact PGSandbox role/database pair, a verification query packet, and the important caveat that PostgreSQL roles are cluster-wide even when the workflow pairs one role with one database.

So the admin URL should be a lifecycle credential, not the credential the agent uses for normal SQL. The safer pattern is:

1. The MCP server uses an admin URL to create a new database and a new role.
2. The server grants that role access only to the sandbox database.
3. The agent receives or uses the sandbox connection for task SQL.
4. Cleanup deletes only the tracked database and role created by the server.

This is the core reason [PGSandbox](https://pgsandbox-mcp.lvtd.dev/) exists. It does not try to make an agent incapable of bad SQL. It gives the agent a smaller place to be wrong.

## 3. Prefer disposable databases over shared development databases

Read-only access is useful, but most coding-agent database work is not purely read-only. Migration validation, seed scripts, bug reproduction, generated SQL review, and demo-state preparation all need writes somewhere.

The unsafe default is to point the agent at a shared development database because it already exists. That is convenient until a migration leaves it half-changed, a test seed collides with another developer, or an interrupted session leaves stale state behind.

A [database sandbox](https://pgsandbox-mcp.lvtd.dev/blog/what-is-database-sandbox/) is the better default for agent work:

- The agent can run real migrations.
- The agent can insert bad seed data without polluting shared state.
- The agent can inspect actual Postgres behavior instead of guessing.
- The task state can be deleted when the work is done.
- The cleanup command can be scoped to resources the tool created.

In PGSandbox, each experiment gets one database, one login role, credentials scoped to that database, TTL metadata, and cleanup tied to tracked resources. The [architecture notes](https://pgsandbox-mcp.lvtd.dev/docs/architecture/) show that resource model in more detail. That is the difference between "the agent can use Postgres" and "the agent can use this Postgres sandbox for this task."

## 4. Make read-only guarantees real

If a Postgres MCP server claims read-only behavior, verify where that guarantee is enforced.

A model instruction is not enforcement. A tool description is not enforcement. Even a server-side read-only wrapper can have implementation bugs. Datadog Security Labs reported a SQL injection vulnerability in Anthropic's reference Postgres MCP server that allowed researchers to bypass a read-only restriction and execute arbitrary SQL (https://securitylabs.datadoghq.com/articles/mcp-vulnerability-case-study-SQL-injection-in-the-postgresql-mcp-server/). The point is not that every server has that bug. The point is that "read-only" must be treated as a property to test, not a phrase to trust.

For a Postgres MCP server, safer read-only design usually includes more than one layer:

- A database role without write privileges.
- Server-side query parsing or transaction controls where appropriate.
- Tool-level separation between inspect, query, create, and delete operations.
- Bounded result sets.
- No direct path from a read-only tool to admin credentials.

PostgreSQL `GRANT` controls privileges on database objects and role membership (https://www.postgresql.org/docs/current/sql-grant.html). Use those database-level controls. They are less ambiguous than a natural-language promise inside a tool description.

## 5. Cap query output and schema exposure

Agents do not need unbounded data dumps to do most backend work.

For migration validation, they usually need schema inspection, row counts, sample rows, and structured error messages. For bug reproduction, they need the failing shape, not every customer record. For generated SQL review, they need the query result and the plan, not a full table export. If the tool fails, the agent should follow a [Postgres MCP server error handling runbook](https://pgsandbox-mcp.lvtd.dev/blog/postgres-mcp-server-error-handling-coding-agents/) that branches on stable codes, categories, SQLSTATE, and hints.

A good Postgres MCP server should make bounded results the default:

- Limit returned rows.
- Truncate long cell values.
- Prefer schema descriptions over table dumps.
- Let the user opt into larger reads deliberately.
- Keep secrets and connection strings out of logs and summaries.

This matters even in local development. Agents copy context around. A "temporary" result can end up in a transcript, a PR comment, or a debugging note. Treat query output as data leaving the database boundary.

## 6. Track ownership before cleanup

Cleanup is only safe when the server knows what it owns.

A generic `DROP DATABASE` tool is too broad. A cleanup tool should delete only databases created by the workflow, preferably with metadata that records the profile, owner, task, creation time, expiry, and resource name. That makes cleanup auditable and prevents the server from guessing based on a name prefix alone.

PGSandbox's model is intentionally narrow: create tracked sandboxes, list tracked sandboxes, delete tracked sandboxes, and clean up expired tracked sandboxes. The destructive operation is scoped to resources PGSandbox created for the selected profile.

For stale resources, the operational split is simple: use metadata-backed cleanup for PGSandbox-created resources, and reserve manual SQL for resources outside that contract. The [cleanup_expired vs manual Postgres cleanup guide](/blog/cleanup-expired-vs-manual-postgres-cleanup/) walks through that decision.

That pattern is worth copying even if you do not use PGSandbox. Before adding a destructive tool to a Postgres MCP server, ask:

- Can it delete anything the server did not create?
- Can it run against the wrong profile?
- Does it support dry-run output?
- Does it show what it will delete before deleting it?
- Does it leave enough metadata to debug cleanup failures?

If the answer is unclear, the tool is not ready for agent use.

## 7. Be careful with cloning realistic data

Realistic data makes agent validation much better. It also raises the stakes.

PostgreSQL's `pg_dump` exports a single database and can make consistent exports without blocking readers or writers (https://www.postgresql.org/docs/current/app-pgdump.html). That makes `pg_dump` and `pg_restore` a practical baseline for cloning source data into a sandbox. But the source still matters.

For agent workflows, the safer clone pattern is:

1. Read from the source database.
2. Restore into a newly-created disposable destination.
3. Run task SQL only against the destination.
4. Delete the destination after the task.

Do not let a coding agent mutate the source database as part of "cloning." If the source is production or production-like, add an explicit human approval step and consider masking or reducing data before the restore.

PGSandbox's clone direction follows that mental model: the source is read, the destination is a tracked PGSandbox-owned database, and restore failure should clean up the destination rather than leaving a broken half-sandbox behind.

For a step-by-step version of that workflow, see [how to clone a Postgres database into a safe sandbox](https://pgsandbox-mcp.lvtd.dev/blog/how-to-clone-postgres-database-sandbox/). It covers the dump/restore flags, schema-only option, source-data boundary, and cleanup rule in one agent-focused flow.

## 8. A practical approval rule

Use this rule for day-to-day work:

For routine coding tasks, approve a Postgres MCP server only when the agent can complete the task inside a disposable database using a scoped role.

That rule keeps the normal path simple. The agent can still get real Postgres behavior. It can still validate migrations and generated SQL. It can still reproduce bugs. But it does not need shared development credentials for every task.

For a step-by-step version of the execution loop, see [how to create a Postgres test database for agent-generated SQL](https://pgsandbox-mcp.lvtd.dev/blog/how-to-create-postgres-test-database-agent-sql/). It covers database creation, scoped roles, migrations, seed data, bounded query results, and cleanup as one proof workflow.

Exceptions should be visible:

- Read-only inspection of a shared development database.
- Production debugging with a human watching.
- Data migration work where the task is explicitly about the source database.
- Performance tuning that needs production-shaped query plans.

Those cases may be valid. They should not be the default.

## FAQ

## Is a Postgres MCP server safe for production?

A Postgres MCP server can be part of a production workflow, but it should not get broad production authority by default. For production use, require strong authorization, audit logs, narrow tool scopes, database roles with least privilege, and explicit human approval for any write or destructive operation.

## Is read-only mode enough?

Read-only mode is useful, but it is not enough by itself. Verify that read-only behavior is enforced by database privileges and server code, not only by an MCP annotation or model instruction. The Datadog Postgres MCP case study is a good reminder that read-only claims can fail at the implementation layer.

## Should coding agents use a shared dev database?

Only when the task truly needs shared state. For migrations, seed data, generated SQL checks, and bug reproductions, a disposable database is usually safer. Shared development databases accumulate stale state and make it harder to tell which task changed what.

## What is the safest default for agent database work?

The safest default is a task-scoped Postgres database with a task-scoped role, bounded query results, metadata-backed cleanup, and no production credentials. That gives the agent real database behavior while limiting the cost of a bad query or interrupted session.

## Where PGSandbox fits

PGSandbox is built for the narrow case this checklist points to: coding agents that need real Postgres proof without being handed a shared database as their workspace. The [install and setup guide](https://pgsandbox-mcp.lvtd.dev/docs/install/) shows the local setup path when you are ready to wire it into Codex, Cursor, VS Code, or Claude Desktop.

It is local-first, private by default, and designed around existing Postgres. The agent gets a tracked disposable database and scoped role for the task. The admin connection is used for lifecycle work. Cleanup targets only resources PGSandbox created.

That is not the only way to run a Postgres MCP server safely. But it is a useful baseline: give agents a real database, make it disposable, and keep the blast radius small.
