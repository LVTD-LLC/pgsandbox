---
title: "How to Create a Postgres Test Database for Agent SQL"
excerpt: "Create a task-scoped Postgres test database when a coding agent needs to run generated SQL against real schema without touching shared development or production state."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-02"
updatedAt: "2026-07-02"
tags: ["Postgres", "test database", "AI agents", "MCP", "SQL validation"]
category: "Engineering"
metaTitle: "Postgres Test Database for Agent SQL"
metaDescription: "Create a Postgres test database for agent-generated SQL with scoped roles, migrations, seed data, bounded queries, and cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/how-to-create-postgres-test-database-agent-sql/"
heroImageUrl: ""
featured: false
sortOrder: 60
---
To create a Postgres test database for agent-generated SQL, create a fresh database for the task, run migrations or load a known source state, give the agent a scoped database role, execute generated SQL only inside that database, capture the result, and delete the database when the task is done.

That is different from handing an agent the same `DATABASE_URL` a human developer uses all week.

Framework test runners already understand this boundary: database-backed tests should run against isolated test databases instead of the real production database. The same rule should apply when the "test runner" is a coding agent generating migrations, seed scripts, or SQL fixes.

The useful pattern is a short-lived proof harness:

1. Create one Postgres database for one agent task.
2. Apply the schema the task needs.
3. Seed only the rows needed to prove the change.
4. Run the agent-generated SQL against a scoped role.
5. Inspect the result and schema diff.
6. Clean up the database and role.

PGSandbox MCP exists to make that loop explicit for MCP clients. The information-gain point is not "use a test database." It is this: agent SQL needs a database lifecycle with authority, state, output, and cleanup boundaries, not just a convenient connection string.

## When an agent needs a Postgres test database

Use a dedicated Postgres test database when the agent's work depends on real database behavior.

Good cases include:

- Testing a generated migration against the current schema.
- Checking an ORM query against real indexes, constraints, and column types.
- Reproducing a bug that depends on several related tables.
- Validating seed scripts or data backfills before a human reviews the patch.
- Letting the agent explore a destructive SQL change away from shared state.

Do not create a database for every question. If the task is only reading model definitions or explaining a query plan, source files and schema docs may be enough. A test database is worth the setup when execution changes the answer.

The important difference is that agent-generated SQL is not just text. Once an MCP server exposes a `run_sql`-style tool, the model has an operational path into state. The [Postgres MCP server safety checklist](https://pgsandbox-mcp.lvtd.dev/blog/postgres-mcp-server-safety-checklist/) covers that broader access review. This guide focuses on the test database itself.

## Step 1: choose the smallest useful state

Start by deciding what database state proves the task.

Use the smallest state that can expose the failure or validate the change:

| State | Use it when | Avoid it when |
| --- | --- | --- |
| Blank database plus migrations | The task is schema, migration, or table-creation work | The bug depends on existing rows |
| Small fixture seed | The query needs representative relationships | The fixture would hide production-shaped edge cases |
| Schema-only clone | The agent needs real object names, constraints, indexes, or extensions | Row-level behavior matters |
| Masked or reduced clone | The bug depends on realistic data shape | Sensitive rows cannot be safely copied |
| Local template sandbox | The same sanitized seed state should be reused across agent tasks | You need live production-shaped data or server-native copy speed |

A blank database is safest, but it is not always honest. A generated SQL query can pass against three clean rows and still fail against nullable legacy data, partial indexes, or enum values that only exist in a long-lived database.

The practical decision is the data shape. If you choose that poorly, the agent can produce a false proof even though every command appears to succeed. For repeatable seeded states, use the [Postgres template database vs task sandbox](https://pgsandbox-mcp.lvtd.dev/blog/postgres-template-database-vs-task-sandbox/) guide to decide whether the state belongs in a native Postgres template, a PGSandbox local template artifact, or a one-off sandbox.

## Step 2: create the database with lifecycle authority

PostgreSQL's `CREATE DATABASE` command creates a new database, and the official docs note that the executing role must be a superuser or have the special `CREATEDB` privilege (https://www.postgresql.org/docs/current/sql-createdatabase.html). That is lifecycle authority. It should not be the same authority the agent uses for task SQL.

The safer shape is:

1. A controlled admin connection creates the database.
2. The workflow creates or selects a task role for the new database.
3. The agent receives only the task database connection.
4. Cleanup uses metadata to delete only resources created by the workflow.

With plain `psql`, that can look like this:

```sql
CREATE DATABASE agent_task_20260702;
CREATE ROLE agent_task_20260702 LOGIN PASSWORD 'replace-with-generated-secret';
GRANT CONNECT ON DATABASE agent_task_20260702 TO agent_task_20260702;
```

Then connect to the new database and grant only what the task needs:

```sql
GRANT USAGE, CREATE ON SCHEMA public TO agent_task_20260702;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO agent_task_20260702;
```

Those statements are intentionally generic. In a real workflow, generate the names and password, avoid logging the password, and store enough metadata to know who owns the test database and when it expires.

PGSandbox MCP handles that lifecycle through its [MCP tool contract](https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/): `create_database` creates an isolated database and login role, returns scoped connection details, and records metadata for later listing and cleanup. The [architecture docs](https://pgsandbox-mcp.lvtd.dev/docs/architecture/) describe the resource model behind that boundary.

## Step 3: apply migrations before SQL validation

A test database is only useful if it matches the schema the generated SQL expects.

For application work, run the same migration command a human would run in a disposable environment. A useful baseline is to create the test database, run migrations to install schema and initial data, run checks or validation queries, and destroy the database when the proof is complete.

For a PGSandbox-backed MCP client, the workflow can be:

1. Create a sandbox database.
2. Run the project migration command against that sandbox.
3. Capture a schema digest before and after the command.
4. Return a compact diff to the agent.

That last piece is the agent-specific improvement. A human can inspect a migration file and a test result. An agent benefits from structured feedback: what changed, which command ran, what stderr said, and whether the schema now matches the expected state.

PGSandbox's repo workflow tools are built around this. The `prepare_repo_workflow` and migration validation flow can infer or store a project command in `.pgsandbox/project.json`, then run bounded commands against the selected sandbox. That keeps the proof tied to a real database without turning the shared development database into the agent's workbench.

## Step 4: seed only what proves the task

Seed data should be small, named, and relevant.

A good agent test database does not need every row. It needs the rows that make the SQL answer meaningful. For example:

- One user with no related records.
- One user with several related records.
- One soft-deleted row if the query filters deleted state.
- One row that violates the naive assumption the agent might make.

If a production-shaped bug really needs realistic data, use a masked source or a reduced clone. PostgreSQL's `pg_dump` utility exports a single database and its docs say it makes consistent backups even while the database is being used concurrently (https://www.postgresql.org/docs/current/app-pgdump.html). PostgreSQL's `pg_restore` docs describe restoring archives created by `pg_dump` and selectively restoring or reordering archive items (https://www.postgresql.org/docs/current/app-pgrestore.html).

That makes dump-and-restore a practical clone path, but it does not make every source safe. If rows are sensitive, the agent should not see them just because a test database is temporary.

When realistic state matters, follow the [Postgres clone database sandbox guide](https://pgsandbox-mcp.lvtd.dev/blog/how-to-clone-postgres-database-sandbox/): read from the source, restore into a newly created sandbox, omit ownership and privilege restoration where possible, and run task SQL only against the destination.

## Step 5: run generated SQL with bounded output

Once the database exists, treat SQL execution as a controlled experiment.

Before the agent runs generated SQL, decide:

1. Which database is the target?
2. Which role is executing?
3. Is the SQL allowed to write?
4. How many rows can come back?
5. What result proves success?

The row limit matters. A generated `SELECT *` against a realistic table can dump far more context than the agent needs. Bounded output is both a safety control and a usability control: the model gets the signal it needs without turning the database into a transcript export.

PGSandbox's `run_sql` tool executes through the sandbox role and returns bounded results. For agent work, that is a better default than exposing a general admin SQL shell. The agent can still write a bad query. It gets a smaller place to be wrong and a smaller result envelope to reason over.

For migrations and generated SQL patches, a useful proof record includes:

- The sandbox database id or name.
- The command or SQL that ran.
- A bounded stdout/stderr or result set.
- The before/after schema objects when schema changed.
- Any errors in structured form.
- Whether cleanup succeeded.

For a migration-specific version of that proof record, see [database migration testing before agent PRs](https://pgsandbox-mcp.lvtd.dev/blog/database-migration-testing-agent-pr/). It focuses on running the repo migration command, capturing schema diffs, seeding risky data cases, and turning the result into a reviewable PR note.

That record is what a human reviewer can trust. "The agent said it tested it" is not enough.

## Step 6: delete the database and role

Cleanup is part of the test, not a janitor task after the test.

PostgreSQL's database creation docs note that the current role becomes the owner of the new database and that the owner can remove it later, including the objects inside it (https://www.postgresql.org/docs/current/manage-ag-createdb.html). That is useful, but it is not sufficient for an agent workflow. The cleanup operation should know exactly which database and role it created.

A safe cleanup path should:

- Delete only databases tracked by the workflow.
- Delete the scoped role connected to that database.
- Refuse to delete arbitrary names the agent invents.
- Support TTL-based cleanup for interrupted sessions.
- Report cleanup failures clearly.

This is where a task-scoped [database sandbox](https://pgsandbox-mcp.lvtd.dev/blog/what-is-database-sandbox/) is cleaner than a manually named test database. The sandbox has an owner, a TTL, labels, and metadata. If the agent stops halfway through a task, the cleanup process still has something concrete to inspect.

## A complete agent SQL proof loop

Here is the operational checklist:

1. Create a fresh Postgres test database for the task.
2. Create a scoped login role for that database.
3. Apply migrations or restore the approved source shape.
4. Seed the smallest useful dataset.
5. Run the generated SQL as the scoped role.
6. Capture bounded output and errors.
7. Compare schema before and after when schema changed.
8. Delete the database and role, or let TTL cleanup catch interrupted runs.

With PGSandbox MCP, the same loop maps to the product surface:

1. Use `create_database` for a blank task database.
2. Use `clone_database` when realistic source state is required.
3. Use repo workflow tools to run migrations or seed commands.
4. Use `run_sql` for bounded SQL execution.
5. Use `describe_schema` or schema digests for structural proof.
6. Use `delete_database` or `cleanup_expired` for cleanup.

That mapping is the practical difference between a generic Postgres test database and a database test harness for coding agents.

## Common mistakes

The first mistake is using a shared development database because it is already configured. Shared state makes agent proof hard to trust. The query may pass because of leftover rows, or fail because another developer changed the same data.

The second mistake is using one powerful connection string for setup, migrations, and agent SQL. PostgreSQL roles are the boundary. Use lifecycle authority to create the database, then task authority to run the work.

The third mistake is cloning more data than the proof needs. A test database can still leak data through prompts, logs, tool outputs, and PR notes. Temporary does not mean harmless.

The fourth mistake is skipping cleanup. A stale test database becomes an undocumented environment. The next agent may discover it, use it, and build a proof on old state.

## FAQ

### Is a Postgres test database the same as a database sandbox?

Not always. A Postgres test database is any database used for test work. A database sandbox has an explicit isolation contract: authority, state, data, time, and cleanup. For agent-generated SQL, that contract matters more than the name.

### Should every agent task get its own database?

No. Use a dedicated database when the task needs execution proof against real Postgres behavior. For documentation edits, code reading, or pure query explanation, a database may add work without improving confidence.

### Can I use Docker instead?

Yes. A containerized Postgres service is a good test boundary for many teams. PGSandbox MCP is useful when you already have a Postgres host or managed local cluster and want the smaller unit to be a task database and role, not a whole new database server. For the tradeoff in detail, read the [Testcontainers vs disposable Postgres sandboxes comparison](https://pgsandbox-mcp.lvtd.dev/blog/testcontainers-vs-disposable-postgres-sandboxes/).

### Should an agent ever test against production?

Routine agent SQL should not run against production. If a production source is needed for a clone, use explicit human approval, a read-only source path, and masking or reduction where possible. The agent should validate against the destination sandbox, not mutate the source.

### What should I link in a pull request?

Link the sandbox proof, not a raw credential. A useful PR note says which sandbox was created, which migration or SQL ran, what result came back, and whether cleanup succeeded. Never paste an unmasked database URL.
