---
title: "How to Use Postgres Schema Snapshots for Agent Migration Reviews"
excerpt: "A practical workflow for using Postgres schema snapshots and diffs as review evidence before an agent opens a migration pull request."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-06"
updatedAt: "2026-07-06"
tags: ["Postgres", "schema diff", "database migrations", "AI agents", "MCP"]
category: "Engineering"
metaTitle: "Postgres Schema Snapshots for Agent Reviews"
metaDescription: "Use Postgres schema snapshots and diffs to review agent migrations with before/after evidence, bounded output, scoped roles, and cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/postgres-schema-snapshots-agent-migration-reviews/"
heroImageUrl: ""
featured: false
sortOrder: 100
---
Postgres schema snapshots help a reviewer see what changed before an agent opens a migration pull request. The useful workflow is to capture a named baseline in a disposable database, run the repo's migration command, diff the current schema against that baseline, then include the changed tables, columns, constraints, indexes, extensions, command result, and cleanup state in the PR.

A schema snapshot is not a replacement for migration tests, seed data, or production rollout review. It is the review artifact that makes the migration legible.

The shortest safe loop is:

1. Create or clone a task-scoped Postgres sandbox.
2. Apply the repo's current schema state.
3. Create a named schema snapshot such as `before_agent_migration`.
4. Run the migration command the repo actually uses.
5. Diff the saved snapshot against the current schema.
6. Add targeted data checks for the risky cases.
7. Delete the sandbox or leave a short TTL with a reason.

The information-gain point is the review contract. A schema diff is not enough by itself. For agent work, a review-grade schema snapshot ties the diff to a specific sandbox id, scoped role, command array, object counts, bounded output, and cleanup path. That is what lets a human reviewer distinguish "the agent says it ran the migration" from "the agent produced database evidence worth reviewing."

PGSandbox exposes that contract directly. The [MCP tool contract](https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/) includes `create_schema_snapshot`, `list_schema_snapshots`, `diff_schema_snapshot`, `schema_digest`, `schema_diff`, and `validate_schema_change`. The [architecture notes](https://pgsandbox-mcp.lvtd.dev/docs/architecture/) describe the resource model behind those tools: one sandbox database, one scoped role, TTL metadata, and cleanup tied to tracked resources.

## What is a Postgres schema snapshot?

A Postgres schema snapshot is a captured description of database structure at a point in time. For migration review, it should include enough metadata to compare tables, columns, constraints, indexes, views, extensions, and other schema objects before and after a change.

PostgreSQL gives you several places to inspect schema state. Its information schema is a set of views for objects in the current database, and the PostgreSQL docs note that it is SQL-standard and stable compared with implementation-specific catalogs (https://www.postgresql.org/docs/current/information-schema.html). The `columns` view includes table schema, table name, column name, ordinal position, default, nullability, data type, identity, and generated-column fields (https://www.postgresql.org/docs/current/infoschema-columns.html).

That is useful, but it is not the whole story for Postgres review. PostgreSQL-specific features often need system catalogs or Postgres-specific views. A review artifact should normalize the important parts rather than ask a reviewer to inspect raw catalog queries.

In PGSandbox, a schema snapshot is a named local checkpoint for a PGSandbox-owned sandbox. It stores a compact schema digest and metadata under the local PGSandbox state directory. It is explicit by design: create a new snapshot when you want a review baseline, diff it when the migration has run, and delete it when it no longer has a job.

## When should an agent use schema snapshots?

Use schema snapshots when the pull request changes database structure and a human reviewer needs a compact before/after record.

Good fits include:

1. A migration adds or removes tables.
2. A migration changes column type, nullability, default, identity, or generated expression.
3. A migration adds indexes, unique constraints, foreign keys, or checks.
4. A migration changes views or materialized views.
5. A bug fix depends on a specific schema shape.
6. A reviewer needs to know whether the migration touched more than the patch claimed.

Do not use snapshots as a way to avoid data checks. A schema snapshot can show that a `NOT NULL` constraint appeared. It cannot prove existing rows satisfy the constraint unless the agent also seeds or checks the relevant data cases.

That is why this topic sits next to the broader [database migration testing workflow](https://pgsandbox-mcp.lvtd.dev/blog/database-migration-testing-agent-pr/). Migration testing proves the command, schema change, data edge cases, and cleanup. Schema snapshots make the schema-change part precise enough to review.

## Schema snapshot vs schema diff vs migration lint

The terms overlap, but they answer different questions.

| Artifact | Question it answers | Review use |
| --- | --- | --- |
| Schema snapshot | What did the database structure look like at this checkpoint? | Baseline before an agent changes the sandbox. |
| Schema diff | What changed between two schema states? | PR summary for tables, columns, constraints, indexes, views, and extensions. |
| Migration lint | Is the migration file likely dangerous? | Early warning for destructive changes, locks, rewrites, or policy violations. |
| Data check | Did representative rows survive the change? | Proof for constraints, backfills, enum changes, and rewritten values. |

External tools reinforce the same pattern. Atlas describes `atlas migrate lint` as analyzing migration files for dangerous or breaking changes, including destructive operations, application-breaking schema changes, table locks, and rewrites (https://atlasgo.io/versioned/lint). Atlas also has `schema diff` workflows for comparing PostgreSQL schemas (https://atlasgo.io/declarative/diff). Stripe's `pg-schema-diff` describes itself as computing differences between Postgres database schemas and generating SQL to move from one schema to another, while warning that not all migrations can avoid locks or downtime (https://github.com/stripe/pg-schema-diff).

Those are strong schema-management tools. PGSandbox has a narrower job: give the coding agent a disposable Postgres target and a bounded evidence path. It does not replace your migration framework or your schema management tool. It gives the agent a safe place to run the framework and a compact way to show what changed.

## Step 1: create the sandbox that owns the proof

Start with a [database sandbox](https://pgsandbox-mcp.lvtd.dev/blog/what-is-database-sandbox/) rather than the shared development database.

The sandbox should have:

1. A short task name.
2. A TTL that matches the review-prep window.
3. An owner label for the agent or session.
4. A scoped role for task SQL.
5. No production connection string in chat, logs, PR notes, or tracked files.

With PGSandbox, `create_database` creates an isolated database and login role. If the migration needs an existing shape, use a schema-only `clone_database` from an approved source or restore a local template into a fresh sandbox. The [Postgres clone database sandbox guide](https://pgsandbox-mcp.lvtd.dev/blog/how-to-clone-postgres-database-sandbox/) explains the clone boundary and why schema-only is often the safer default.

The important rule is authority separation. The tool may need lifecycle authority to create the database. The migration command should run against the sandbox connection, not against a general admin URL.

## Step 2: capture the baseline snapshot

After the sandbox has the pre-change schema, create a named snapshot.

Use names that describe the review stage:

```text
before_agent_migration
before_backfill_index
before_pr_1842
before_payment_status_constraint
```

The name matters because agents lose context. A named checkpoint is easier to inspect than "the schema from earlier." The [MCP tool docs](https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/) show the same pattern: create a schema snapshot before the change, then diff that snapshot after the migration command runs.

A good snapshot response should give the reviewer enough shape without dumping raw catalog output:

```text
Snapshot
- Sandbox: pgsandbox_orders_7f3a
- Snapshot: before_payment_status_constraint
- Objects: 24 tables, 131 columns, 18 constraints, 34 indexes, 2 extensions
- Digest version: <schema digest version>
- Created: 2026-07-06T06:00:00Z
```

The exact counts will vary. The point is that the baseline is named, attached to a sandbox, and compact.

## Step 3: run the real migration command

Run the command the repository uses, not an improvised SQL fragment.

For some projects that is `npm run db:migrate`. For others it is a Rails, Django, Ecto, Prisma, Flyway, Atlas, Sqitch, Drizzle, Alembic, or custom SQL command. The agent should use the repo's actual command path because the proof needs to exercise what the team will run later.

Frameworks already understand part of this problem. Prisma's documentation says its shadow database is a temporary database used by `prisma migrate dev` to detect schema drift and potential data loss, and that it compares the expected migration-history state with the development database (https://www.prisma.io/docs/orm/prisma-migrate/understanding-prisma-migrate/shadow-database). That is useful inside Prisma's migration workflow.

The agent still needs a task-level proof outside the framework. A reviewer may not only ask whether the migration framework accepted the change. They may ask what changed in Postgres, whether an index became invalid, whether a constraint appeared, whether the agent used the right database, and whether cleanup happened.

When possible, pass the command as an argv array rather than a shell string:

```json
["npm", "run", "db:migrate"]
```

That makes the proof clearer and reduces accidental shell behavior. PGSandbox's repo-oriented workflow tools use explicit command arrays for this reason.

## Step 4: diff the snapshot against the current schema

After the migration command runs, diff the current sandbox schema against the named snapshot.

A useful diff groups changes by review object:

```text
Schema diff
- Added tables: public.audit_events
- Changed tables: public.users
- Added columns: public.users.deleted_at
- Changed constraints: public.users.users_email_key
- Added indexes: public.users_users_deleted_at_idx
- Removed objects: none
- Truncated: false
```

That summary is short enough for a PR body and specific enough for review.

PGSandbox's schema digest intentionally ignores sandbox database identity when computing schema shape. That matters for agent workflows because a reviewer usually cares whether two task databases have the same structure, not whether they have the same generated database name.

The diff should also call out limits. If the result was truncated because the migration touched many objects, say that. A bounded diff is safer than an unbounded dump, but a truncated diff is not complete proof. The agent should attach a detail handle, rerun a narrower inspection, or summarize the missing category.

## Step 5: look for the dangerous Postgres cases

Schema snapshots are most valuable when they make dangerous changes visible.

Postgres index operations are a good example. PostgreSQL documents that a failed concurrent index build can leave behind an invalid index that is ignored for queries but still consumes update overhead, and recommends dropping it and trying again or using `REINDEX INDEX CONCURRENTLY` in some cases (https://www.postgresql.org/docs/current/sql-createindex.html). The `REINDEX` docs describe a similar invalid-index recovery path for failed concurrent rebuilds (https://www.postgresql.org/docs/current/sql-reindex.html).

A reviewer should not have to infer that from a green command exit. The snapshot diff and follow-up checks should make it explicit:

```text
Index check
- public.users_email_idx: valid
- public.orders_created_at_idx: valid
- Invalid indexes after migration: none
```

Other review-sensitive changes include:

1. A column becoming `NOT NULL`.
2. A default changing on a live table.
3. A unique constraint appearing before duplicate rows are checked.
4. A foreign key appearing without representative parent and child rows.
5. A view definition changing while application code still expects the old shape.
6. An extension being added or removed.
7. A table rewrite or lock-sensitive operation that belongs in a rollout plan.

The snapshot catches the schema side. Targeted SQL catches the data side.

## Step 6: add data checks only for the risky cases

Do not paste full table dumps into a PR.

Use small, bounded checks that map to the migration risk:

```sql
select count(*) as duplicate_emails
from (
  select email
  from users
  where deleted_at is null
  group by email
  having count(*) > 1
) duplicates;
```

or:

```sql
select count(*) as null_payment_status_rows
from invoices
where payment_status is null;
```

PGSandbox's `run_sql` returns bounded rows and supports row limits. That is the right shape for PR evidence. The reviewer needs counts, representative failures, and SQLSTATE details, not sensitive rows or a huge transcript.

If a schema change is meant to alter a query path, add a [Postgres EXPLAIN plan review](https://pgsandbox-mcp.lvtd.dev/blog/postgres-explain-plan-agent-sql/) beside the bounded row checks. The plan shows whether Postgres expects to touch the intended relations before the agent treats execution output as proof.

If the task needs realistic source shape, start with schema-only, synthetic, masked, or reduced data. A disposable sandbox limits where writes land. It does not make sensitive data safe to expose.

## Step 7: write the PR evidence block

The final output should be boring and reviewable.

Use this format:

```text
Schema snapshot validation
- Sandbox: pgsandbox_<task>_<id>, scoped role <role>, TTL <minutes>
- Command: ["npm", "run", "db:migrate"]
- Baseline snapshot: before_agent_migration
- Changed objects: 1 table changed, 1 column added, 1 index added, 0 removed
- Risk checks: no duplicate emails, no invalid indexes, no null payment statuses
- Data output: bounded row counts only
- Cleanup: sandbox deleted / TTL cleanup pending until <time>
```

This block gives a reviewer a clear place to start. It also gives the agent a fail condition. If the diff shows an unexpected object, the migration is not ready for review. If cleanup did not happen, the PR should say why.

## Common mistakes

The first mistake is snapshotting the wrong database. A baseline from a shared development database tells you very little about what the agent changed. Snapshot the task sandbox.

The second mistake is keeping only a checksum. A checksum can prove that two schema states differ, but it does not tell the reviewer what changed. Keep the compact object diff too.

The third mistake is treating schema proof as data proof. A diff can show that a unique constraint was added. It cannot prove there are no duplicate rows unless you run the check.

The fourth mistake is hiding the command. A before/after diff without the command leaves the reviewer guessing how the database got from one state to the other.

The fifth mistake is leaving snapshots and sandboxes behind. A snapshot is a checkpoint for a task, not a permanent audit database. Delete it or let the tracked sandbox cleanup path handle it.

## FAQ

### Is a schema snapshot the same as a database backup?

No. A schema snapshot captures database structure for comparison. It is not a data backup, restore point, or production recovery artifact. Use backup tooling for data recovery.

### Can I use schema snapshots with Prisma, Rails, Django, or Ecto?

Yes. The snapshot sits around the framework command. Create the baseline in a sandbox, run the framework's migration command against that sandbox, then diff the current schema against the baseline.

### Should every agent PR include a schema snapshot?

No. Use snapshots for database-structure changes. A UI-only or docs-only PR does not need one. A migration PR, generated SQL change, or backend bug fix that depends on schema shape usually does.

### What if the diff is empty?

An empty diff can be useful if the task was supposed to be data-only or idempotent. If the PR claims to change schema and the diff is empty, the agent probably ran the wrong command, targeted the wrong database, or produced a migration that did not apply.

### Does PGSandbox replace migration lint tools?

No. Use migration lint tools when they fit your stack. PGSandbox gives agents a disposable database, scoped role, snapshot/diff workflow, bounded SQL output, and cleanup path so the lint result can be paired with real Postgres evidence.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "FAQPage",
  "mainEntity": [
    {
      "@type": "Question",
      "name": "Is a schema snapshot the same as a database backup?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "No. A schema snapshot captures database structure for comparison. It is not a data backup, restore point, or production recovery artifact. Use backup tooling for data recovery."
      }
    },
    {
      "@type": "Question",
      "name": "Can I use schema snapshots with Prisma, Rails, Django, or Ecto?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "Yes. The snapshot sits around the framework command. Create the baseline in a sandbox, run the framework's migration command against that sandbox, then diff the current schema against the baseline."
      }
    },
    {
      "@type": "Question",
      "name": "Should every agent PR include a schema snapshot?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "No. Use snapshots for database-structure changes. A UI-only or docs-only PR does not need one. A migration PR, generated SQL change, or backend bug fix that depends on schema shape usually does."
      }
    },
    {
      "@type": "Question",
      "name": "What if the diff is empty?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "An empty diff can be useful if the task was supposed to be data-only or idempotent. If the PR claims to change schema and the diff is empty, the agent probably ran the wrong command, targeted the wrong database, or produced a migration that did not apply."
      }
    },
    {
      "@type": "Question",
      "name": "Does PGSandbox replace migration lint tools?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "No. Use migration lint tools when they fit your stack. PGSandbox gives agents a disposable database, scoped role, snapshot/diff workflow, bounded SQL output, and cleanup path so the lint result can be paired with real Postgres evidence."
      }
    }
  ]
}
</script>
