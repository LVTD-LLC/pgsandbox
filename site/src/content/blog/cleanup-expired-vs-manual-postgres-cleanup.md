---
title: "cleanup_expired vs Manual Postgres Cleanup for Agent Sandboxes"
excerpt: "Compare PGSandbox cleanup_expired with manual Postgres cleanup when coding agents leave stale task databases, roles, and proof state behind."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-12"
updatedAt: "2026-07-12T06:00:00Z"
tags: ["Postgres", "MCP", "cleanup", "database sandbox", "agent safety"]
category: "Engineering"
metaTitle: "cleanup_expired vs Manual Postgres Cleanup"
metaDescription: "Compare cleanup_expired with manual Postgres cleanup for stale agent sandboxes, scoped roles, TTL metadata, dry runs, and recovery workflows."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/cleanup-expired-vs-manual-postgres-cleanup/"
heroImageUrl: ""
featured: false
sortOrder: 132
---
Use `cleanup_expired` when the resource was created by PGSandbox and still exists in PGSandbox metadata. Use manual Postgres cleanup only as a scoped fallback when the metadata path is unavailable, corrupted, or you need to remove resources outside PGSandbox's ownership boundary.

That is the practical answer for stale coding-agent sandboxes. `cleanup_expired` is a safer default because it selects from tracked sandbox records, supports dry runs, and preserves the same owner/label/TTL model used at creation time. Manual SQL is powerful, but it shifts the burden back to the operator: prove the database, role, active connections, ownership, and dependencies before dropping anything.

Here is the short version:

| Cleanup job | Better fit | Why |
| --- | --- | --- |
| Remove expired PGSandbox-created task databases | `cleanup_expired` | It works from metadata and can preview selected resources with `dryRun`. |
| Clean up one known sandbox after a task finishes | `delete_database` or `cleanup_expired` | Use the database id when you have it; use TTL cleanup for expired leftovers. |
| Sweep stale resources across owners or labels | `cleanup_expired` | Owner and label filters keep cleanup scoped to the workflow. |
| Drop an untracked scratch database | Manual Postgres cleanup | PGSandbox should not delete resources it did not create. |
| Recover from broken metadata or profile access | Manual Postgres cleanup after diagnosis | The metadata path may be unavailable, but the SQL path still needs explicit scope. |

The information-gain point is this: cleanup is not just "drop the old database." For coding agents, cleanup is part of the proof boundary. A good cleanup path can say which task owned the database, why it was selected, whether it was expired, what was deleted, and what still needs follow-up.

## What `cleanup_expired` actually controls

PGSandbox exposes `cleanup_expired` as an MCP tool that deletes expired resources with dry-run support for audit-friendly cleanup (https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/).

The architecture docs define the underlying boundary: each sandbox gets one database, one login role, scoped credentials, a TTL, and optional labels. Cleanup can run through the explicit MCP tool or a scheduled process, and it only deletes databases listed in metadata and matching the configured prefix (https://pgsandbox-mcp.lvtd.dev/docs/architecture/).

Those two details are the reason `cleanup_expired` is the default for stale PGSandbox resources:

1. The selection set comes from tracked sandbox state, not a name pattern somebody typed during an incident.
2. The result can be tied back to workflow fields such as owner, labels, profile, expiration time, and database id.
3. The dry-run path lets an agent or reviewer inspect selected resources before deletion.

Manual SQL can remove a database. It cannot, by itself, tell you whether the database was created for the current task, whether it belongs to another agent, or whether its TTL had expired.

## What manual Postgres cleanup controls

Manual cleanup means using native PostgreSQL commands and utilities directly. The usual operations are:

- inspect databases and roles,
- terminate or wait for active connections,
- run `DROP DATABASE`,
- clean up role-owned objects and privileges,
- run `DROP ROLE` after dependencies are gone.

PostgreSQL's current `DROP DATABASE` reference says the command removes the database catalog entries and data directory, can only be executed by the database owner, cannot run while connected to the target database, and cannot be undone (https://www.postgresql.org/docs/current/sql-dropdatabase.html).

That is a useful primitive, but it is intentionally low-level. PostgreSQL also says `DROP DATABASE` cannot run inside a transaction block, and `FORCE` can try to terminate active connections but will still fail when prepared transactions, active logical replication slots, or subscriptions block termination (https://www.postgresql.org/docs/current/sql-dropdatabase.html).

Role cleanup has its own dependency path. PostgreSQL documents that dropping a role often requires owned objects to be reassigned or dropped and privileges to be revoked first. The general recipe is `REASSIGN OWNED`, then `DROP OWNED`, repeated in each database that contains objects owned by the role, before `DROP ROLE` (https://www.postgresql.org/docs/current/role-removal.html).

That is the operational cost of manual cleanup: each destructive command is only one part of the cleanup story.

## The decision matrix

| Dimension | `cleanup_expired` | Manual Postgres cleanup |
| --- | --- | --- |
| Selection source | PGSandbox metadata: database id, owner, labels, TTL, profile | Operator queries, role names, database names, conventions |
| Preview mode | Built-in `dryRun` | Manual `SELECT` queries and review discipline |
| Scope | PGSandbox-created resources only | Any database or role the operator can access |
| Best use | Expired task sandboxes, interrupted agent sessions, scheduled hygiene | Untracked resources, metadata recovery, emergency operator repair |
| Main safety risk | Bad owner/label policy or stale profile metadata | Dropping the wrong database, role dependency errors, active connection surprises |
| Proof artifact | Selected, deleted, failures, profile follow-up | Manually written command log and query output |
| Product boundary | MCP lifecycle tool for coding agents | Direct database administration |

For agent work, the selection source is the deciding row. If a tool can select resources from metadata, prefer that over reconstructing intent from names.

## Use `cleanup_expired` when metadata is intact

Use `cleanup_expired` when the stale database came from PGSandbox and the profile can still read its metadata.

Good examples:

1. A coding-agent session was interrupted before `delete_database`.
2. A task intentionally set a short TTL and left the sandbox for review.
3. A shared profile has expired resources from several agents.
4. A local Postgres version sweep needs to clean expired resources across profiles.
5. A PR proof note needs to show cleanup status without exposing a connection string.

Start with a dry run:

```json
{
  "tool": "cleanup_expired",
  "arguments": {
    "owner": "agent-session",
    "labels": {
      "repo": "pgsandbox-mcp"
    },
    "dryRun": true
  }
}
```

Then run the same scope without `dryRun` only after the selected resources match the expected task boundary.

This maps directly to the agent workflow docs: list or clean up one profile by default, and use all-version scope deliberately with `includeAllVersions` and `dryRun` when the task really needs it (https://github.com/LVTD-LLC/pgsandbox-mcp/blob/main/docs/agent-workflows.md).

## Use manual cleanup when the resource is outside the contract

Manual cleanup is still the right answer for resources PGSandbox should not own.

Good examples:

1. A developer created a scratch database outside PGSandbox.
2. A previous prototype used a different naming convention and no metadata.
3. PGSandbox metadata is unavailable and `doctor` confirms profile access is broken.
4. You need to repair a role dependency before PGSandbox can delete the tracked role.
5. A human operator intentionally needs broader Postgres administration outside an MCP client.

When you go manual, write the proof before the destructive command:

```sql
-- inspect the candidate first
SELECT datname, pg_catalog.pg_get_userbyid(datdba) AS owner
FROM pg_database
WHERE datname = 'pgsandbox_task_20260712_old';

-- check connections before drop
SELECT pid, usename, application_name, state
FROM pg_stat_activity
WHERE datname = 'pgsandbox_task_20260712_old';
```

Only then should you move to `DROP DATABASE`, from a different database such as `postgres`, with an operator role that is allowed to drop the target.

## Why manual role cleanup is easy to under-scope

The sharp edge in manual cleanup is often the role, not the database.

A PGSandbox sandbox has a scoped login role tied to one task database. If you created that role manually, you need to prove what it owns and where. PostgreSQL's role-removal docs call this out: owned objects and privileges may exist across databases, and cleanup commands may need to run in each database where the role owns objects (https://www.postgresql.org/docs/current/role-removal.html).

`REASSIGN OWNED` changes ownership of objects owned by one or more roles to a new role in the current database. PostgreSQL also notes that it does not affect objects in other databases and does not revoke privileges granted to the old role on objects it did not own; `DROP OWNED` handles the privilege cleanup path (https://www.postgresql.org/docs/current/sql-reassign-owned.html).

For a coding-agent cleanup loop, that is too much implicit state to hand-wave. If the role was created by PGSandbox, let the metadata-backed delete/cleanup path handle it. If the role was not created by PGSandbox, keep the manual runbook explicit and local to a human-reviewed operation.

## A practical cleanup policy for shared agent machines

Use this policy when several agents or repos share local Postgres profiles:

| Situation | Policy |
| --- | --- |
| Agent creates a sandbox for a task | Set `owner`, stable labels, and a TTL. |
| Task finishes successfully | Prefer `delete_database` with the database id. |
| Task is interrupted or times out | Run `cleanup_expired` with `dryRun` by owner and label. |
| Multiple Postgres versions are active | Use profile-specific cleanup first; widen to `includeAllVersions` only after preview. |
| Metadata cleanup reports failures | Treat failures as follow-up work, not as success. |
| Resource is not in metadata | Use manual cleanup with an operator-reviewed query log. |

That policy gives agents a narrow path most of the time and leaves manual SQL for cases where an operator actually needs database-admin authority.

For a concrete taxonomy, use the [owner and label policy for shared PGSandbox profiles](/blog/owner-label-policy-shared-pgsandbox-profiles/). It turns the table above into repeatable `owner`, `repo`, `task`, `workflow`, and `retention` fields that cleanup can select safely.

## What a PR-ready cleanup note should include

For `cleanup_expired`, include the selection and result:

```text
Cleanup:
- method: cleanup_expired
- scope: owner=agent-session, labels.repo=pgsandbox-mcp
- dryRunReviewed: true
- selected: 2
- deleted: 2
- failures: 0
- remainingProfiles: none
```

For manual cleanup, include the checks that made it safe:

```text
Manual cleanup:
- target: pgsandbox_task_20260712_old
- owner verified: yes
- active connections checked: yes
- dependency cleanup needed: no
- command: DROP DATABASE from postgres maintenance database
- reason cleanup_expired was not used: resource was not tracked in PGSandbox metadata
```

The second note is longer because manual cleanup has to carry the context that metadata would normally provide.

## Common mistakes

### Mistake: using names as the cleanup boundary

A name prefix is useful evidence, not proof. Prefer metadata fields when they exist. For manual cleanup, pair name checks with owner, age, connection, and dependency checks.

### Mistake: using manual SQL because it feels faster

`DROP DATABASE` may be quick, but the verification around it is not optional. If the database is tracked by PGSandbox, `cleanup_expired` plus `dryRun` is usually the faster safe path.

### Mistake: treating `FORCE` as a cleanup plan

`FORCE` is a connection-handling option, not a resource-selection policy. PostgreSQL documents cases where it still will not terminate blockers such as prepared transactions, active logical replication slots, or subscriptions.

### Mistake: deleting databases but leaving roles behind

Manual cleanup often leaves roles, grants, and owned objects as follow-up work. PGSandbox-created resources should go through the PGSandbox delete/cleanup path so the database and role lifecycle stay together.

### Mistake: skipping labels

Labels are the difference between "clean up whatever this owner touched" and "clean up the expired resources for this repo/task class." For shared machines, labels are part of the safety model.

## Related pages

- [How to Use cleanup_expired for Stale PGSandbox Resources](/blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/)
- [Owner and Label Policy for Shared PGSandbox Profiles](/blog/owner-label-policy-shared-pgsandbox-profiles/)
- [Postgres MCP Server Safety Checklist for Coding Agents](/blog/postgres-mcp-server-safety-checklist/)
- [How to Run Agent SQL with Bounded Postgres Results](/blog/postgres-run-sql-bounded-results/)
- [MCP tool contract](/docs/mcp-tools/)
- [Architecture](/docs/architecture/)
