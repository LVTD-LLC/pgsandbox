---
title: "How to Use Postgres EXPLAIN Plans for Agent SQL Review"
excerpt: "Use Postgres EXPLAIN plans to review agent-generated SQL before execution: create a sandbox, inspect the JSON plan, check row estimates, then run bounded proof only when mutation is intentional."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-07"
updatedAt: "2026-07-07"
tags: ["Postgres", "EXPLAIN", "query plans", "AI agents", "MCP"]
category: "Engineering"
metaTitle: "Postgres EXPLAIN Plans for Agent SQL Review"
metaDescription: "Review agent-generated SQL with Postgres EXPLAIN plans, JSON output, scoped sandboxes, row estimates, bounded proof, and cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/postgres-explain-plan-agent-sql/"
heroImageUrl: ""
featured: false
sortOrder: 100
---
To review agent-generated SQL with a Postgres EXPLAIN plan, run the query plan inside a disposable sandbox before executing the SQL against shared state. Use `EXPLAIN (FORMAT JSON)` for machine-readable evidence, check the scan and join shape, compare estimated rows against the expected task, and only run bounded SQL proof after the plan matches the intent.

That workflow matters because a coding agent can produce syntactically valid SQL that is still wrong for the database. It may scan more rows than expected, miss an obvious index, join through the wrong relation, or turn a small review task into a broad data read. A plan review gives the human and the agent a compact checkpoint before execution becomes the proof.

The agent-safe sequence is:

1. Create or clone a task-scoped Postgres sandbox.
2. Load the schema and smallest useful fixture.
3. Ask Postgres for a JSON EXPLAIN plan.
4. Review relation names, node types, estimated rows, and total cost.
5. Fix the query or schema if the plan contradicts the task.
6. Run [`run_sql` with bounded Postgres results](https://pgsandbox-mcp.lvtd.dev/blog/postgres-run-sql-bounded-results/) using `readonly: true` and a small `rowLimit` for read proof.
7. Use an intentional mutation path only after review.
8. Delete the sandbox or let TTL cleanup handle it.

The information-gain point is the review contract. For agent work, a Postgres EXPLAIN plan is not just a tuning artifact. It is a pre-execution safety record: what the agent intended to ask Postgres to do, which relations Postgres planned to touch, and whether that plan is narrow enough to continue.

## What does a Postgres EXPLAIN plan show?

A Postgres EXPLAIN plan shows the execution plan the PostgreSQL planner generates for a statement. The official `EXPLAIN` docs describe the plan as the table scan methods, join algorithms, and other execution steps Postgres expects to use (https://www.postgresql.org/docs/current/sql-explain.html).

For human review, that plan answers practical questions:

- Is this a targeted index lookup or a broad sequential scan?
- Is the query touching the tables the task actually named?
- Are estimated rows close to the expected shape?
- Is the join strategy plausible for the data size?
- Is the plan narrow enough to run in a sandbox proof loop?

Postgres also documents that machine-readable EXPLAIN output formats such as JSON, XML, and YAML are better when another program needs to inspect the plan (https://www.postgresql.org/docs/current/using-explain.html). That is the right default for coding agents. Text plans are readable, but JSON plans can be summarized, stored in a PR note, compared, and passed between tools without asking the model to parse indentation.

PGSandbox's `explain_query` tool uses that shape directly. The [MCP tool contract](https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/) documents `explain_query` as returning `EXPLAIN (FORMAT JSON)` for one safe plannable statement, plus a compact summary of node types, relations, cost, and estimated rows. It does not use `ANALYZE`, rejects multi-statement SQL, and rejects transaction or session controls.

That is deliberately narrower than a general SQL shell.

## EXPLAIN is not the same as EXPLAIN ANALYZE

`EXPLAIN` plans a statement. `EXPLAIN ANALYZE` executes it and reports actual runtime information.

That difference is the first safety boundary. PostgreSQL's `EXPLAIN` documentation says the `ANALYZE` option causes the statement to be actually executed, not only planned (https://www.postgresql.org/docs/current/sql-explain.html). For a `SELECT`, that may be acceptable inside a sandbox. For `INSERT`, `UPDATE`, `DELETE`, `MERGE`, or DDL-shaped work, executing the statement just to inspect it is the wrong default.

For agent SQL review, start with non-executing EXPLAIN:

```sql
EXPLAIN (FORMAT JSON)
SELECT id, email
FROM accounts
WHERE id = 42;
```

Then decide whether to run a bounded proof query:

```json
{
  "databaseId": "sandbox-id",
  "sql": "SELECT id, email FROM accounts WHERE id = 42",
  "readonly": true,
  "rowLimit": 5
}
```

That split keeps the review clean. The EXPLAIN plan says what Postgres intends to do. The bounded read proof says what rows came back. A mutation should have a separate, explicit reason and a task database where the damage is contained.

The [Postgres test database guide](https://pgsandbox-mcp.lvtd.dev/blog/how-to-create-postgres-test-database-agent-sql/) covers the broader proof loop for generated SQL. EXPLAIN belongs near the front of that loop, before the agent treats execution output as validation.

## Step 1: create a sandbox before planning agent SQL

Do not ask an agent to inspect a risky query against production or a shared development database.

Create a [database sandbox](https://pgsandbox-mcp.lvtd.dev/blog/what-is-database-sandbox/) first. The sandbox should have the schema shape the query needs, the smallest useful fixture data, a scoped role, a TTL, and a cleanup path. If realistic schema or approved sample data matters, clone or seed it into the sandbox instead of pointing the agent at the source database.

PGSandbox's current resource model is one database, one scoped login role, TTL metadata, and cleanup tied to tracked resources. The [architecture docs](https://pgsandbox-mcp.lvtd.dev/docs/architecture/) describe that model, and the [MCP tool docs](https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/) document the lifecycle tools around it.

This is the first review checkpoint:

| Question | Good answer |
| --- | --- |
| What database is the plan using? | A PGSandbox-created sandbox id or name |
| What credentials will execution use? | The sandbox role, not the admin URL |
| What data is present? | Empty, seeded, cloned, or templated state that is approved for the task |
| How long can it live? | A short TTL appropriate for the review |
| How does it disappear? | `delete_database` or metadata-backed cleanup |

The plan only means something if it is produced against a database shape close enough to the real task. A plan over an empty schema can catch syntax and relation errors, but it cannot tell you whether the row estimates make sense for realistic data.

## Step 2: ask for one statement, not a script

Plan review works best when the agent explains one statement at a time.

PGSandbox's `explain_query` accepts one statement. Multi-statement SQL fails with `single_statement_required`, because a plan proof should not hide several actions in one blob. That matches the human review habit: inspect the exact query that matters, not a transcript with setup, mutation, and verification mixed together.

Good input:

```sql
SELECT o.id, o.total_cents
FROM orders o
JOIN accounts a ON a.id = o.account_id
WHERE a.email = 'review@example.com'
ORDER BY o.created_at DESC
LIMIT 10;
```

Bad input:

```sql
SET statement_timeout = '5s';
SELECT o.id, o.total_cents
FROM orders o
JOIN accounts a ON a.id = o.account_id
WHERE a.email = 'review@example.com';
DELETE FROM orders WHERE account_id IS NULL;
```

The second input is not a query plan. It is a script with session control and mutation hidden beside inspection. The right move is to split the work:

1. Plan the `SELECT`.
2. Review the plan summary.
3. Run a bounded read proof if needed.
4. Treat mutation as a separate, intentional step.

The official MCP tools specification says servers must validate tool inputs and implement proper access controls, while clients should show tool inputs to the user and prompt for confirmation on sensitive operations (https://modelcontextprotocol.io/specification/2025-06-18/server/tools). A one-statement EXPLAIN contract is a small version of that rule applied to SQL review.

## Step 3: read the plan like a reviewer

An agent does not need to become a DBA to use an EXPLAIN plan well. It needs a repeatable checklist.

Start with these fields:

| Plan signal | What to ask |
| --- | --- |
| Node types | Is Postgres scanning, joining, sorting, aggregating, or modifying what the task expects? |
| Relation names | Are the touched tables the intended tables? |
| Estimated rows | Is the planner expecting one row, a small set, or most of the table? |
| Total cost | Did a small lookup become a broad expensive plan? |
| Join shape | Does the plan join through the intended keys? |
| Filter conditions | Did the filter land where the agent thought it would land? |

PostgreSQL's planner depends on statistics about table contents. The planner statistics docs explain that Postgres stores statistics and uses them to estimate rows and choose plans (https://www.postgresql.org/docs/current/planner-stats.html). The `ANALYZE` command collects those statistics for tables and stores them for planner use (https://www.postgresql.org/docs/current/sql-analyze.html).

That means bad estimates can have several causes:

- The sandbox has no representative rows.
- The table statistics are stale or missing.
- The query predicate does not match the intended index.
- The agent wrote a broader join than the task needed.
- The schema in the sandbox differs from the target environment.

Do not treat every sequential scan as a bug. A sequential scan over a tiny fixture may be fine. Do treat mismatched relations, unexpected row estimates, and missing filters as reasons to stop and inspect the query before execution.

## Step 4: use bounded SQL proof after the plan passes

After the plan passes review, run a [bounded proof query](https://pgsandbox-mcp.lvtd.dev/blog/postgres-run-sql-bounded-results/).

PGSandbox's `run_sql` executes through sandbox role credentials, not the admin connection. Its docs define a default `rowLimit` of 100, allow `rowLimit: 0` for a zero-row preview, cap returned rows at 1000, and return ordered per-statement result sets for multi-statement SQL (https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/).

For read proof, use `readonly: true`:

```json
{
  "databaseId": "sandbox-id",
  "sql": "SELECT count(*) AS matching_accounts FROM accounts WHERE status = 'active'",
  "readonly": true,
  "rowLimit": 10
}
```

Readonly mode is not a replacement for review. It is the execution boundary after review. The PGSandbox docs explain that `readonly: true` runs SQL in a read-only transaction, rejects transaction-control escape hatches, rolls back after execution, and returns `readonly_violation` for mutating statements such as `INSERT` or `CREATE TEMP TABLE`.

That gives the PR reviewer a compact evidence trail:

```text
Sandbox: pgsandbox_fix_account_status_ab12cd34
Plan: EXPLAIN JSON touched accounts through an index lookup, estimated 1 row
Read proof: count(*) returned 1 matching row, rowLimit 10, readonly true
Cleanup: deleted sandbox after review
```

The useful part is not the exact wording. It is the separation: plan, read proof, cleanup. Each line answers a different review question.

## Step 5: use EXPLAIN with migrations carefully

Migration work needs a different proof shape.

For schema changes, a query plan can be useful when the migration adds an index, rewrites a query, or changes a constraint that affects lookup behavior. But an EXPLAIN plan is not enough to prove a migration. A migration proof should still capture before and after schema state, risky data rows, command output, and cleanup.

Use the migration workflow first:

1. Create a sandbox.
2. Capture the before schema.
3. Run the repo migration command.
4. Capture the after schema.
5. Diff the schema.
6. Run targeted EXPLAIN plans for affected queries.
7. Run bounded data checks.
8. Clean up.

The [database migration testing workflow](https://pgsandbox-mcp.lvtd.dev/blog/database-migration-testing-agent-pr/) covers that PR gate. The [schema snapshot guide](https://pgsandbox-mcp.lvtd.dev/blog/postgres-schema-snapshots-agent-migration-reviews/) goes deeper on named before/after checkpoints and compact diffs.

EXPLAIN fits inside that workflow when the review question is performance or access path:

- Did the new index change the planned lookup?
- Did a constraint or join rewrite affect estimated rows?
- Did the query still touch the expected table after a rename?
- Did a view change expand into a more expensive plan than expected?

Do not use EXPLAIN as a substitute for applying the migration. A plan can tell you how Postgres expects to execute a statement. It cannot prove the migration command succeeds, that old rows survive, or that cleanup happened.

## Common mistakes when agents use EXPLAIN

The mistakes are predictable.

### Mistake 1: running EXPLAIN ANALYZE first

`EXPLAIN ANALYZE` executes the statement. In a sandbox, it can be useful after review. As a first move, it is too eager. Start with non-executing EXPLAIN, then decide whether the task needs actual runtime evidence.

### Mistake 2: planning against the wrong data shape

A plan from an empty sandbox can be misleading. If the query is about a production-shaped join, seed the minimum rows that make the join meaningful or clone an approved source into a [safe sandbox](https://pgsandbox-mcp.lvtd.dev/blog/how-to-clone-postgres-database-sandbox/). Keep the source read-only and run the query only against the destination.

### Mistake 3: pasting a huge text plan into the PR

Use JSON for tool-to-tool handling and summarize the review-relevant parts for humans. A PR should not need a full planner dump unless the details are central to the change.

### Mistake 4: trusting row estimates as facts

Estimated rows are estimates. They are still useful because they show the planner's current belief. If estimates are wildly wrong, check fixture data, statistics, predicates, and indexes before treating the result as proof.

### Mistake 5: mixing plan proof with mutation proof

Keep the evidence separate. The EXPLAIN plan answers "what would Postgres do?" The bounded SQL result answers "what happened when we ran a safe read?" The mutation or migration proof answers "what changed?" Combining those into one transcript makes review harder.

## A PR-ready evidence block

Use a short block like this when an agent changes SQL:

```text
Database proof:
- Sandbox: <database id/name>, scoped role, TTL <minutes>
- Plan: explain_query on <query name>; node types <...>; relations <...>; estimated rows <...>
- Read proof: run_sql readonly=true, rowLimit=<n>; result <summary>; truncated=<true/false>
- Mutation proof, if any: <repo command or intentional SQL path>
- Cleanup: delete_database succeeded or TTL cleanup scheduled
```

For a query-only patch, the mutation line should often be "none." That is a useful answer. It tells the reviewer the agent did not need broad write access to prove the work.

## The practical rule

Use Postgres EXPLAIN plans as a pre-execution review gate for agent SQL.

If the plan touches the right relations, has plausible row estimates, and matches the task's intent, run bounded proof in the sandbox. If the plan surprises you, stop there. Fix the query, fixture, schema, or statistics before execution becomes the evidence.

PGSandbox MCP's current product shape supports that division of labor: `explain_query` for one safe plannable statement, `run_sql` for bounded sandbox execution, schema tools for migration proof, and metadata-backed cleanup for the task database. That is the boundary coding agents need: enough database access to prove work, not enough ambiguity to make review impossible.
