---
title: "How to Run Agent SQL with Bounded Postgres Results"
excerpt: "Run agent-generated SQL against a disposable Postgres sandbox with readonly mode, row limits, typed result sets, and a PR-ready evidence record."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-08"
updatedAt: "2026-07-08"
tags: ["Postgres", "SQL", "AI agents", "MCP", "query results"]
category: "Engineering"
metaTitle: "Run Agent SQL with Bounded Postgres Results"
metaDescription: "Run agent SQL safely with PGSandbox MCP: scoped sandbox roles, readonly mode, row limits, typed result sets, and cleanup proof."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/postgres-run-sql-bounded-results/"
heroImageUrl: ""
featured: false
sortOrder: 110
---
To run agent-generated SQL with bounded Postgres results, create or clone a disposable sandbox, execute the SQL through the sandbox role, set `readonly: true` for read proof, choose a small `rowLimit`, inspect the typed result metadata, and delete the sandbox when the task is finished.

That gives the agent real database feedback without turning the database into an unbounded transcript source. The important boundary is specific: every SQL proof should name the target database, execution authority, read/write intent, returned row budget, result interpretation, and cleanup state.

A useful agent SQL proof loop looks like this:

1. Create a task-scoped [database sandbox](https://pgsandbox-mcp.lvtd.dev/blog/what-is-database-sandbox/).
2. Load the schema or safe source state the task needs.
3. Review the query shape with `explain_query` when the SQL is non-trivial.
4. Run `run_sql` with `readonly: true` for read proof.
5. Set `rowLimit` to the smallest result that proves the task.
6. Read `returnedRowCount`, `truncated`, and `resultSets` alongside the rows.
7. Run intentional mutation only when the task actually needs it.
8. Delete the sandbox or leave an explicit TTL cleanup path.

The information-gain point is the result contract. For coding agents, bounded SQL output is not a convenience feature. It is the review surface that converts "the agent queried the database" into a small, inspectable proof record.

## Why bounded Postgres results matter for agents

Bounded Postgres results matter because an agent can generate a valid `SELECT` that returns far more context than the task needs. Even inside a sandbox, a broad result set wastes tokens, hides the important row, and can expose data the reviewer did not intend to place in the agent context.

PostgreSQL's `LIMIT` docs say a limit count returns no more than that many rows, and they also warn that predictable subsets require `ORDER BY` (https://www.postgresql.org/docs/current/queries-limit.html). That matters for agent proof. A result limit controls volume. It does not, by itself, make the returned rows deterministic.

For reviewable SQL proof, use both:

```sql
SELECT id, email, status
FROM accounts
WHERE status = 'pending_review'
ORDER BY id
LIMIT 5;
```

Then also set the MCP result envelope limit:

```json
{
  "databaseId": "sandbox-id",
  "readonly": true,
  "rowLimit": 5,
  "sql": "SELECT id, email, status FROM accounts WHERE status = 'pending_review' ORDER BY id LIMIT 5"
}
```

The SQL `LIMIT` expresses task intent. The MCP `rowLimit` is the output budget. Keeping both visible makes the proof easier to audit.

## Step 1: run SQL only inside a task sandbox

Do not point a coding agent at the same Postgres database a human developer uses all week.

Create or clone a task database first. In PGSandbox MCP, that means a tracked sandbox with one database, one scoped login role, TTL metadata, and cleanup tied to resources PGSandbox created. The [architecture docs](https://pgsandbox-mcp.lvtd.dev/docs/architecture/) describe that resource model, and the [MCP tool contract](https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/) lists the lifecycle tools around it.

The target should be specific:

| Field | Good proof value |
| --- | --- |
| Database | PGSandbox-created sandbox id or name |
| Role | Sandbox role, not admin URL |
| State | Empty, migrated, seeded, cloned, or templated |
| TTL | Short enough for the review task |
| Cleanup | `delete_database` result or TTL cleanup note |

That first table is not paperwork. It tells the reviewer where the SQL ran and which authority executed it. If the agent cannot answer those questions, the result rows are not enough evidence.

## Step 2: decide whether this is read proof or mutation proof

Before running SQL, classify the task.

Most agent SQL checks are read proof. The agent is trying to answer a question such as "does this query return the expected row?" or "does the migration leave the expected data shape?" For those, use `readonly: true`.

PGSandbox's docs define `run_sql` as executing through the sandbox role with optional readonly mode and capped row limits (https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/). With `readonly: true`, PGSandbox starts a read-only transaction, rejects transaction-control escape hatches, rolls back after execution, and returns `readonly_violation` for mutating statements such as `INSERT` or `CREATE TEMP TABLE`.

That maps to Postgres itself. PostgreSQL documents `BEGIN` with `READ ONLY` as a transaction mode (https://www.postgresql.org/docs/current/sql-begin.html). Its `SET TRANSACTION` docs explain that a read-only transaction disallows commands such as `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `CREATE`, `ALTER`, `DROP`, `TRUNCATE`, and write-capable `COPY FROM` (https://www.postgresql.org/docs/current/sql-set-transaction.html).

Use mutation proof only when the task actually needs to write. Examples:

- Applying a migration.
- Testing an `UPDATE ... RETURNING` patch.
- Seeding rows that prove a follow-up read.
- Reproducing a bug that depends on a write path.

When mutation is intentional, make it explicit in the proof record. Do not let a write hide in a "just checking" query.

## Step 3: choose the row limit before execution

Choose the smallest `rowLimit` that can prove the task.

PGSandbox's current `run_sql` contract uses these boundaries:

| `rowLimit` value | Meaning |
| --- | --- |
| Omitted | Use the default of 100 rows |
| `0` | Valid zero-row preview |
| `1` through `1000` | Return up to that many rows |
| Negative | Return `invalid_row_limit` |
| Above `1000` | Cap at 1000 |

Those boundaries come from the repo docs and implementation. The cap is deliberately small for agent workflows. A coding agent usually needs a count, a few sample rows, or a targeted failure case, not thousands of rows.

Good examples:

```json
{
  "databaseId": "sandbox-id",
  "readonly": true,
  "rowLimit": 0,
  "sql": "SELECT * FROM accounts WHERE status = 'pending_review' ORDER BY id"
}
```

Use `rowLimit: 0` when you want to validate that the statement runs and inspect metadata without returning rows.

```json
{
  "databaseId": "sandbox-id",
  "readonly": true,
  "rowLimit": 3,
  "sql": "SELECT id, status, updated_at FROM accounts WHERE status = 'pending_review' ORDER BY updated_at DESC"
}
```

Use a tiny positive limit when sample rows are the proof.

```json
{
  "databaseId": "sandbox-id",
  "readonly": true,
  "rowLimit": 1,
  "sql": "SELECT count(*) AS pending_count FROM accounts WHERE status = 'pending_review'"
}
```

Use aggregates when the proof is a number. A count with `rowLimit: 1` is often more useful than a broad sample.

## Step 4: read the full result envelope

The result rows are only one part of the proof.

PGSandbox's `run_sql` output includes:

- `rows`: rows from the last row-returning statement.
- `resultSets`: ordered per-statement results.
- `returnedRowCount`: number of rows included in `rows`.
- `affectedRowCount`: affected rows for DML or DDL command tags when applicable.
- `totalRowCountKnown`: whether total row count is known without inference.
- `truncated`: whether output was bounded by `rowLimit`.
- `elapsedMs`: execution timing.

The `resultSets` array matters for multi-statement SQL. The PGSandbox docs say each result set includes a 1-based `statementIndex`, row counts, affected counts, and truncation state. The top-level `rows` mirror the last row-returning statement, which is useful for a quick answer but incomplete when several statements ran.

For agent review, ask the model to summarize the envelope like this:

```text
SQL proof:
- Target: <sandbox id/name>
- Mode: readonly=true
- Row budget: rowLimit=3
- Statements: 2 result sets
- Returned: 3 rows from statement 2
- Truncated: true
- Interpretation: query matched expected pending accounts; sample is bounded
```

That is better than pasting raw JSON into a PR comment. The reviewer sees the important controls and can ask for the full result only if needed.

## Step 5: handle multi-statement SQL deliberately

Multi-statement SQL is a sharp tool for agents.

PostgreSQL's libpq docs note that a command string can include multiple SQL commands separated by semicolons, and that the result object describes only the last command in that string for that API (https://www.postgresql.org/docs/current/libpq-exec.html). PGSandbox improves the agent-facing shape by splitting multi-statement SQL into ordered `resultSets`, but the review concern remains: several statements can hide several different intentions.

Prefer one statement when the proof question is one statement.

Use multi-statement SQL when the setup and proof are tightly related, for example:

```sql
SET search_path TO public;
SELECT current_schema() AS schema_name;
```

Avoid mixing setup, mutation, and verification into one opaque blob:

```sql
CREATE TABLE scratch_accounts(id integer);
INSERT INTO scratch_accounts VALUES (1);
SELECT * FROM scratch_accounts;
DROP TABLE scratch_accounts;
```

That script may be fine inside a sandbox when mutation is the task. It is not read proof. It should run with `readonly: false`, a clear reason, and a cleanup expectation.

## Step 6: use EXPLAIN before broad execution

If the SQL is non-trivial, inspect the plan before execution.

The [Postgres EXPLAIN plan guide](https://pgsandbox-mcp.lvtd.dev/blog/postgres-explain-plan-agent-sql/) covers the review step in detail. The short version: use `explain_query` to check relation names, node types, row estimates, and whether the query shape matches the task before running a bounded proof query.

That sequence keeps evidence separate:

| Evidence | Tool | Question answered |
| --- | --- | --- |
| Plan | `explain_query` | What will Postgres likely do? |
| Read result | `run_sql` with `readonly: true` | What rows or aggregate came back? |
| Mutation result | `run_sql` or repo workflow with intentional writes | What changed? |
| Schema result | `schema_digest`, `schema_diff`, snapshots | What database objects changed? |
| Cleanup | `delete_database` or `cleanup_expired` | Did the task resource disappear? |

For migration work, combine this with the [database migration testing workflow](https://pgsandbox-mcp.lvtd.dev/blog/database-migration-testing-agent-pr/). A query result is not a migration proof by itself.

## Common mistakes when agents run SQL

The mistakes are predictable and easy to prevent.

### Mistake 1: treating `rowLimit` as the only safety control

`rowLimit` bounds returned rows. It does not make the SQL read-only, choose the right database, hide sensitive columns, or produce deterministic order. Use a sandbox, scoped role, readonly mode, selected columns, `ORDER BY`, and cleanup together.

### Mistake 2: returning `SELECT *`

Ask for the columns that prove the task. `SELECT *` makes the proof larger and can include fields the review does not need. A useful agent query names the identifier, the changed field, and one or two context fields.

### Mistake 3: omitting `ORDER BY`

Postgres warns that `LIMIT` without `ORDER BY` gives an unpredictable subset. For agent proof, unpredictability is noise. Add an order that matches the review question.

### Mistake 4: ignoring `truncated`

If `truncated` is true, the result is a sample, not the whole answer. That may be fine. Say so. If the reviewer needs completeness, use `count(*)`, a narrower predicate, or a larger but still intentional limit.

### Mistake 5: retrying readonly failures as writes without a reason

A `readonly_violation` is a useful stop sign. Retry with `readonly: false` only when mutation is intended and the target is still a sandbox. The proof should name the reason.

## A PR-ready bounded SQL evidence block

Use a compact block when the agent changes SQL or validates database behavior:

```text
Database proof:
- Sandbox: <database id/name>, scoped role, TTL <minutes>
- State: <empty/migrated/seeded/cloned/template>
- Plan, if reviewed: <summary from explain_query>
- SQL mode: readonly=<true/false>, rowLimit=<n>
- Result: returnedRowCount=<n>, truncated=<true/false>, affectedRowCount=<n/null>
- Interpretation: <one sentence explaining why this proves the task>
- Cleanup: <delete_database succeeded / TTL cleanup scheduled>
```

This is the practical line: agent SQL needs bounded output because the output becomes part of the review. The smaller and more explicit that record is, the easier it is for a human to trust.

PGSandbox MCP's `run_sql` contract is built for that job. It gives the agent real Postgres execution inside a disposable sandbox, with readonly mode, capped row results, typed result sets, and metadata-backed cleanup. That is enough database access to prove work without making the SQL transcript the new source of risk.
