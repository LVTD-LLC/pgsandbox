---
title: "How to Test pg_stat_statements in Agent Sandboxes"
excerpt: "Test pg_stat_statements preload setup, database registration, task-role visibility, query capture, clone behavior, and cleanup in disposable Postgres sandboxes."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-21"
updatedAt: "2026-07-21T06:00:00Z"
tags: ["Postgres", "pg_stat_statements", "query performance", "database testing", "coding agents"]
category: "Engineering"
metaTitle: "Test pg_stat_statements in Agent Sandboxes"
metaDescription: "Test pg_stat_statements preload setup, task-role query capture, database registration, clone behavior, and cleanup in a disposable Postgres sandbox."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-pg-stat-statements-agent-sandboxes/"
heroImageUrl: ""
featured: false
sortOrder: 143
---
To test `pg_stat_statements` safely, configure it on the target PostgreSQL server, restart that server, register the extension in a disposable database, run a recognizable workload as the sandbox role, and prove that the same role can read the expected normalized query statistics. Keep cluster setup separate from task-database setup, and delete the sandbox after the evidence is captured.

That separation is the important part. `pg_stat_statements` spans two scopes that are easy to blur: the module allocates shared state at PostgreSQL server startup, while its SQL views and functions are installed in individual databases. A coding agent should validate both scopes instead of retrying `CREATE EXTENSION` until something happens.

PGSandbox does not add a special `pg_stat_statements` command. Its [MCP tool contract](/docs/mcp-tools/) provides the smaller lifecycle pieces: inspect profiles and extensions, create a scoped database, run bounded SQL, and remove tracked resources. Server configuration remains an explicit operator or profile concern.

## In this guide

- [The five boundaries to prove](#the-five-boundaries-to-prove)
- [Configure the PostgreSQL profile](#1-configure-the-postgresql-profile)
- [Create and inspect a sandbox](#3-create-a-sandbox-with-pg_stat_statements)
- [Run a recognizable query probe](#5-run-a-recognizable-query-probe)
- [Handle permissions and clone behavior](#7-keep-the-task-role-narrow)
- [Troubleshoot failed tests](#pg_stat_statements-troubleshooting-matrix)
- [Capture PR-ready evidence](#pr-ready-pg_stat_statements-proof-packet)

## pg_stat_statements sandbox workflow at a glance

Use this sequence:

1. Resolve the exact PostgreSQL profile and confirm the extension files are available.
2. Add `pg_stat_statements` to `shared_preload_libraries`, enable query IDs, and restart PostgreSQL.
3. Verify the running server reports the expected preload state.
4. Create a disposable sandbox with the extension registered in that database.
5. Run a unique, repeatable workload as the sandbox role.
6. Query `pg_stat_statements` for that database, role, and workload shape.
7. Record bounded evidence, then delete the sandbox.

This workflow tests what the application or agent will actually use. Seeing `pg_stat_statements` in `pg_available_extensions` proves that supporting files are visible to the server. It does not prove the module was preloaded, registered in the target database, or usable through the intended task role.

## The five boundaries to prove

A useful test treats `pg_stat_statements` as five related states. This article calls them the **Observability Boundary Contract**.

| Boundary | Question | Evidence |
| --- | --- | --- |
| Profile | Does the selected PostgreSQL installation expose the extension files? | `list_extensions` or `pg_available_extensions` |
| Cluster | Did the running server preload the module and enable query IDs? | `SHOW shared_preload_libraries` and `SHOW compute_query_id` |
| Database | Is the extension registered in the task database? | `pg_extension` and sandbox-scoped `list_extensions` |
| Workload | Did the sandbox role generate and read the expected normalized query entry? | Filtered `pg_stat_statements` rows |
| Lifecycle | Can the test finish without copying irrelevant observability state or leaving a database behind? | Clone exclusion result and tracked deletion |

Most setup guides cover the first three rows. Agent testing needs all five. The workload boundary catches permission and query-shape mistakes. The lifecycle boundary prevents a short observability check from turning into another shared, long-lived database.

## Why shared_preload_libraries changes the test

The PostgreSQL 18 [`pg_stat_statements` documentation](https://www.postgresql.org/docs/current/pgstatstatements.html) says the module needs `shared_preload_libraries` because it allocates additional shared memory. Adding or removing the module therefore requires a server restart. Query identifier calculation must also be active; PostgreSQL enables it automatically when `compute_query_id` is `auto` or `on` unless another module supplies query IDs.

This is server startup state. A sandbox database cannot make it true with SQL inside that database. `CREATE EXTENSION pg_stat_statements` installs the views and functions into the current database, but it does not restart the server or rewrite the profile's preload configuration.

That gives the agent a clean recovery rule:

- If the extension is unavailable, fix the selected PostgreSQL installation or choose a configured profile.
- If PostgreSQL reports a preload requirement, configure and restart the profile outside the task database.
- If preload is active but the view is missing, register the extension in the sandbox database.
- If the view exists but the probe is absent, inspect role visibility, query filters, and collection settings.

PGSandbox maps recognized preload failures to `extension_setup_required`. The [error-handling guide](/blog/postgres-mcp-server-error-handling-coding-agents/) explains why an agent should branch on that stable code instead of retrying blindly.

## 1. Configure the PostgreSQL profile

First choose the PostgreSQL profile or major version that matches the environment you want to test. Run `list_profiles`, then inspect extension availability before creating a database:

```json
{
  "postgresVersion": "18"
}
```

Use that input with `list_extensions`. A matching `availableExtensions` entry proves the selected server can see the extension control and supporting files. PostgreSQL's [`CREATE EXTENSION` reference](https://www.postgresql.org/docs/current/sql-createextension.html) is explicit that those files must already be installed before a database can load an extension.

Next, configure the selected PostgreSQL server:

```ini
shared_preload_libraries = 'pg_stat_statements'
compute_query_id = on
```

Preserve any other libraries already present in `shared_preload_libraries`; it is a list, not a slot reserved for this module. Then restart the correct PostgreSQL instance. The exact command depends on how that profile is managed, so keep the restart in the profile's documented operator workflow rather than embedding host administration in an agent prompt.

Do not hand a broad server-admin credential to the coding agent for this step. Profile configuration changes affect every database on that PostgreSQL server and belong in an operator-reviewed setup path.

## 2. Verify the running server state

After the restart, query the selected profile or a short-lived database on it:

```sql
SHOW shared_preload_libraries;
SHOW compute_query_id;
```

The first result should include `pg_stat_statements`. The second should be `auto` or `on`, unless the profile deliberately loads another query-ID provider.

Also inspect server-visible availability:

```sql
SELECT name, default_version, installed_version
FROM pg_available_extensions
WHERE name = 'pg_stat_statements';
```

These checks answer different questions. `SHOW shared_preload_libraries` reports the running server configuration. `pg_available_extensions` reports files available for database registration. Neither one alone proves that a task database has installed the extension.

## 3. Create a sandbox with pg_stat_statements

Create a disposable database on the configured profile and request the extension:

```json
{
  "postgresVersion": "18",
  "nameHint": "pgss query proof",
  "ttlMinutes": 90,
  "owner": "agent:query-review",
  "labels": {
    "repo": "example-api",
    "workflow": "pg-stat-statements"
  },
  "extensions": ["pg_stat_statements"]
}
```

Use that input with `create_database`. PGSandbox checks `pg_available_extensions` in the new target, then runs `CREATE EXTENSION IF NOT EXISTS` through the generated sandbox role. If extension installation fails, database creation is rolled back so the task does not continue with an ambiguous half-configured sandbox.

This is where the cluster and database boundaries meet. The operator has prepared the server; the agent asks for the database-local object it needs. The returned sandbox role remains the credential for task SQL. The admin profile stays on the lifecycle side of the boundary described in the [PGSandbox architecture](/docs/architecture/).

If the request returns `extension_setup_required`, stop. Confirm that you restarted the profile selected by the request, not a different PostgreSQL instance. If it returns `invalid_extensions`, confirm the package files exist for that exact server major and installation.

## 4. Verify database-local registration

Inspect the newly created sandbox with `list_extensions` using its `databaseId` or `databaseName`. The result should include `pg_stat_statements` under `installedExtensions`.

You can cross-check the database catalog with bounded read-only SQL:

```sql
SELECT extname, extversion
FROM pg_extension
WHERE extname = 'pg_stat_statements';
```

Then inspect the collector metadata:

```sql
SELECT dealloc, stats_reset
FROM pg_stat_statements_info;
```

The information view gives context that a bare `SELECT * FROM pg_stat_statements` misses. PostgreSQL documents `dealloc` as the number of times least-executed entries were discarded after the collector saw more distinct statements than `pg_stat_statements.max`. `stats_reset` records the last full reset. Capture both when a test depends on a clean or stable observation window.

Do not assume the agent can reset global statistics. PostgreSQL restricts `pg_stat_statements_reset` to superusers by default, although an administrator can grant it separately. A task-role test is usually safer when it creates a recognizable query shape and filters by database and user instead of resetting shared collector state.

## 5. Run a recognizable query probe

Create a small table whose name makes the test workload easy to isolate:

```sql
CREATE TABLE pgss_probe_agent_review (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  status text NOT NULL
);

INSERT INTO pgss_probe_agent_review (status)
VALUES ('queued'), ('done'), ('done');
```

Run the same parameterizable query several times as the sandbox role:

```sql
SELECT count(*)
FROM pgss_probe_agent_review
WHERE status = 'done';
```

`pg_stat_statements` normalizes structurally identical statements so literal values can share one entry. The collector keys entries by database ID, user ID, query ID, and whether the statement is top-level. A good proof therefore checks the query shape and its scope rather than looking for one literal SQL string copied from a log.

Use a bounded inspection query:

```sql
SELECT
  query,
  calls,
  rows,
  total_exec_time,
  mean_exec_time
FROM pg_stat_statements
WHERE dbid = (
  SELECT oid FROM pg_database WHERE datname = current_database()
)
  AND userid = (
    SELECT usesysid FROM pg_user WHERE usename = current_user
  )
  AND query ILIKE '%pgss_probe_agent_review%'
ORDER BY total_exec_time DESC
LIMIT 10;
```

The expected evidence is not a benchmark number. It is a row for the probe query, a call count consistent with the number of executions, the correct database/user scope, and timing fields that can be compared only within a controlled test. Do not turn a tiny disposable fixture into a production performance claim.

For broader query-plan review, pair this collector check with the [Postgres EXPLAIN workflow](/blog/postgres-explain-plan-agent-sql/). `EXPLAIN` shows the plan for a specific statement; `pg_stat_statements` aggregates what actually ran. They answer related but different questions.

## 6. Interpret collection settings before comparing results

The default `pg_stat_statements.track` value is `top`, so nested statements executed inside functions are not collected unless the profile uses `all`. PostgreSQL also defaults `pg_stat_statements.track_planning` to `off` and warns that enabling it can add a noticeable performance penalty under some concurrent workloads.

Before comparing two runs, record the settings that change what gets collected:

```sql
SHOW pg_stat_statements.track;
SHOW pg_stat_statements.track_utility;
SHOW pg_stat_statements.track_planning;
SHOW pg_stat_statements.max;
```

A missing nested query under `track = top` is not proof that the module failed. Zero planning time while planning tracking is off is also expected. The proof packet should name the settings that matter so a reviewer does not interpret configuration differences as application regressions.

Use `pg_stat_statements_info.dealloc` when the observed workload contains many distinct query shapes. A rising value means entries have been evicted because the collector exceeded `pg_stat_statements.max`; it does not mean PostgreSQL lost every statistic.

## 7. Keep the task role narrow

PostgreSQL allows ordinary users to see their own query text and query IDs. Seeing SQL text and query IDs for other users requires superuser authority or membership in `pg_read_all_stats`. That boundary is useful for agent sandboxes.

If the application query and the inspection query both run as the sandbox role, the agent can usually prove its own workload without a cluster-wide monitoring role. Do not grant `pg_read_all_stats` merely to make a test query more convenient. That role expands visibility across users on the server and weakens the task boundary.

If the application deliberately uses a second role, decide what the test needs:

- Run the collector assertion through an approved monitoring identity outside the agent task.
- Grant narrowly reviewed access for the test profile only.
- Or keep the proof scoped to queries executed by the sandbox role.

Record the choice. A blank query field caused by role visibility is different from a collector that failed to record calls.

## 8. Handle clone sources explicitly

An existing source database may already register `pg_stat_statements`, but its statistics are not application schema state that a task clone needs to reproduce. Preload state belongs to the target PostgreSQL server, and the SQL views/functions should be registered intentionally in the target database.

PGSandbox therefore skips source `pg_stat_statements` extension entries during `clone_database` by default. This prevents `pg_restore` from trying to recreate a source observability extension through a sandbox role that may not have the required profile setup or privileges.

For a clone-based test, use the two extension controls for their separate jobs:

- `extensions` requests database-local extensions that should exist in the target before restore.
- `excludeSourceExtensions` adds other source-only extension entries that should be omitted; `pg_stat_statements` is already in the default exclusion set.

Only request `pg_stat_statements` in `extensions` when the target profile has already been configured and the test genuinely needs query collection. The [disposable extension testing guide](/blog/test-postgres-extensions-locally/) covers the broader availability, installation, behavior, migration, and cleanup gates.

## 9. Delete the sandbox and preserve compact evidence

After the probe and inspection queries pass, delete the tracked sandbox with `delete_database`. If a session is interrupted, the TTL and `cleanup_expired` provide a second cleanup path, but explicit deletion should remain the normal end of a successful test.

Keep the PR evidence small:

- resolved PostgreSQL profile and major;
- extension availability and installed version;
- preload and query-ID settings;
- probe query shape and expected call count;
- filtered collector row count and relevant timing fields;
- `dealloc` and `stats_reset` context;
- role used for workload and inspection;
- sandbox deletion result.

Do not include admin URLs, sandbox passwords, or full connection strings. The [bounded SQL guide](/blog/postgres-run-sql-bounded-results/) shows how to keep agent-generated database evidence compact and secret-free.

## pg_stat_statements troubleshooting matrix

| Symptom | Likely boundary | Check | Next action |
| --- | --- | --- | --- |
| `pg_stat_statements` absent from `pg_available_extensions` | Profile | Selected major and package installation | Install supporting files for that PostgreSQL installation or choose another profile |
| `extension_setup_required` | Cluster | `SHOW shared_preload_libraries` on the selected profile | Add the module, restart the correct server, and retry |
| Relation `pg_stat_statements` does not exist | Database | `pg_extension` in the sandbox | Register the extension in that database |
| View exists but probe query is missing | Workload | `dbid`, `userid`, track mode, query shape | Run the probe as the intended role and narrow the filter |
| Query text is blank for another role | Authority | Current user and `pg_read_all_stats` membership | Keep the test self-scoped or use an approved monitoring identity |
| Planning columns stay zero | Configuration | `pg_stat_statements.track_planning` | Treat as expected when off; enable only with an explicit reason |
| Expected entries disappear | Capacity | `pg_stat_statements_info.dealloc` and `pg_stat_statements.max` | Reduce test noise or review collector capacity with the operator |
| Clone restore fails around an observability extension | Lifecycle | Source TOC and target profile | Keep source `pg_stat_statements` excluded; request target registration only when configured |

## Common mistakes

### Treating availability as activation

An available extension has supporting files the server can see. It may still be absent from `shared_preload_libraries` or unregistered in the sandbox database. Verify all three states.

### Restarting the wrong PostgreSQL instance

Local machines often have several majors, containers, or services. Resolve the profile first, then run `SHOW shared_preload_libraries` against that exact server after restart.

### Resetting shared statistics for a task test

A full reset affects shared collector evidence and normally needs superuser authority. Prefer a unique probe table and filters for the current database and role.

### Comparing timing from unrealistic fixtures

`pg_stat_statements` measures what ran, but a three-row sandbox is not production-shaped. Use it to prove capture, permissions, query normalization, and regression mechanics. Use approved representative data and controlled conditions before drawing performance conclusions.

### Restoring source observability state blindly

The target server decides preload state. The target database decides registration. Letting a source archive decide both makes clone behavior fragile and hides the authority boundary.

## PR-ready pg_stat_statements proof packet

Use this template:

```text
pg_stat_statements proof
- Profile / PostgreSQL major: <profile> / <major>
- Available / installed extension: <yes/no> / <version>
- shared_preload_libraries: <redacted setting without secrets>
- compute_query_id: <auto/on/provider>
- Collector settings: track=<value>, planning=<value>, max=<value>
- Workload role: <sandbox role name or stable label>
- Probe: <query shape>, expected calls=<n>
- Observed: rows=<n>, calls=<n>, dealloc=<n>, stats_reset=<timestamp>
- Clone behavior: <not used / source pg_stat_statements excluded>
- Cleanup: <database id deleted / expiry fallback recorded>
```

The packet proves the boundaries without pretending that one short run is a benchmark. It also gives a reviewer enough state to reproduce a missing-query or permission failure.

## Frequently asked questions

### Does pg_stat_statements need shared_preload_libraries?

Yes. PostgreSQL requires `pg_stat_statements` in `shared_preload_libraries` because the module allocates shared memory at server startup. Changing that list requires a PostgreSQL restart. Database-local `CREATE EXTENSION pg_stat_statements` installs the views and functions but does not replace the preload step.

### Is CREATE EXTENSION enough to enable pg_stat_statements?

No. `CREATE EXTENSION` registers the SQL objects in the current database. The server must already have the supporting files, preload the module, and calculate query IDs. Verify profile availability, running server state, and database registration separately.

### Can a sandbox role read pg_stat_statements?

A normal role can inspect its own query text and query IDs through the installed view. SQL text and query IDs for other users require superuser authority or `pg_read_all_stats`. Keep an agent test scoped to its sandbox role unless broader monitoring access has been reviewed explicitly.

### Why does PGSandbox exclude pg_stat_statements from clones?

PGSandbox excludes the source extension entry by default because observability registration and preload state belong to the target environment. A source archive should not force a sandbox role to recreate a server-dependent extension. Request target registration explicitly only on a configured profile.

### Should I use pg_stat_statements or EXPLAIN for agent SQL review?

Use both when the task needs both perspectives. `EXPLAIN` inspects the plan for a specific statement without requiring aggregate history. `pg_stat_statements` reports normalized statistics for statements that actually ran. Neither one substitutes for representative data or application-level correctness tests.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Article",
      "headline": "How to Test pg_stat_statements in Agent Sandboxes",
      "description": "Test pg_stat_statements preload setup, database registration, task-role visibility, query capture, clone behavior, and cleanup in disposable Postgres sandboxes.",
      "datePublished": "2026-07-21",
      "dateModified": "2026-07-21",
      "author": {"@type": "Organization", "name": "PGSandbox Team"},
      "publisher": {"@type": "Organization", "name": "PGSandbox MCP"},
      "mainEntityOfPage": "https://pgsandbox-mcp.lvtd.dev/blog/test-pg-stat-statements-agent-sandboxes/"
    },
    {
      "@type": "HowTo",
      "name": "Test pg_stat_statements in a disposable agent sandbox",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Resolve the PostgreSQL profile", "text": "Choose the target PostgreSQL profile and confirm pg_stat_statements supporting files are available."},
        {"@type": "HowToStep", "position": 2, "name": "Configure the server", "text": "Add pg_stat_statements to shared_preload_libraries, enable query IDs, and restart the selected PostgreSQL server."},
        {"@type": "HowToStep", "position": 3, "name": "Create the sandbox", "text": "Create a disposable database and request database-local pg_stat_statements registration."},
        {"@type": "HowToStep", "position": 4, "name": "Run a query probe", "text": "Execute a recognizable, repeatable workload as the sandbox role."},
        {"@type": "HowToStep", "position": 5, "name": "Inspect bounded statistics", "text": "Filter pg_stat_statements by the current database, role, and probe query shape."},
        {"@type": "HowToStep", "position": 6, "name": "Record and clean up", "text": "Capture compact settings and query evidence, then delete the tracked sandbox."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "Does pg_stat_statements need shared_preload_libraries?", "acceptedAnswer": {"@type": "Answer", "text": "Yes. PostgreSQL requires the module in shared_preload_libraries and a server restart when that setting changes. CREATE EXTENSION separately installs its views and functions in a database."}},
        {"@type": "Question", "name": "Is CREATE EXTENSION enough to enable pg_stat_statements?", "acceptedAnswer": {"@type": "Answer", "text": "No. The supporting files must be installed, the server must preload the module and calculate query IDs, and the extension must be registered in the target database."}},
        {"@type": "Question", "name": "Can a sandbox role read pg_stat_statements?", "acceptedAnswer": {"@type": "Answer", "text": "A normal role can inspect its own query text and query IDs. Seeing query text and query IDs for other users requires superuser authority or pg_read_all_stats."}},
        {"@type": "Question", "name": "Why does PGSandbox exclude pg_stat_statements from clones?", "acceptedAnswer": {"@type": "Answer", "text": "PGSandbox excludes the source extension entry because preload and database registration belong to the target environment. Request target registration explicitly only when the selected profile is configured."}}
      ]
    }
  ]
}
</script>
