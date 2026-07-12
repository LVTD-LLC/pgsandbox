---
title: "How to Use cleanup_expired for Stale PGSandbox Resources"
excerpt: "Run `cleanup_expired` in dry-run mode first, target stale sandboxes precisely with owner and labels, and keep interrupted workflows from leaking task databases."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-11"
updatedAt: "2026-07-11T09:00:00Z"
tags: ["Postgres", "MCP", "postgres sandboxes", "cleanup", "agent safety"]
category: "Engineering"
metaTitle: "Use cleanup_expired for Stale PGSandbox Databases"
metaDescription: "A practical PGSandbox guide for running cleanup_expired safely: dry-run first, owner/label filters, cross-version cleanup, and safe recovery when resources stay behind."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/"
heroImageUrl: ""
featured: false
sortOrder: 131
---
When PGSandbox sandboxes stop deleting themselves, stale task databases are the most common source of hidden local debt. The cleanup path is not one command line at the end: it is a small control surface with the same boundaries and metadata model as creation.

**Direct answer for this question**: Use `cleanup_expired` for TTL cleanup, but run it as `dryRun` first, and only then remove what is truly stale and scoped to resources PGSandbox created.

The official contract for `cleanup_expired` is explicit: use `dryRun` to preview matches, then run without `dryRun` to delete those expired resources (https://pgsandbox.lvtd.dev/docs/mcp-tools/).

## What `cleanup_expired` is designed to handle

PGSandbox’s design starts with a metadata-first model:

- every sandbox is a tracked database + role + owner/purpose/labels,
- each has `createdAt` and `expiresAt`,
- destructive actions are expected to be metadata-bound, not name-based.

The architecture notes and cleanup section say cleanup should only delete databases listed in metadata and matching the configured prefix (https://pgsandbox.lvtd.dev/docs/architecture/).

That matters for two reasons:

1. **You can recover safely**. Cleanup has enough context to prove what it selected.
2. **You do not delete arbitrary databases** if a human typo lands on a command string.

`cleanup_expired` also supports owner and label filters, which keeps cleanup actionable in shared environments where multiple agents or workflows share the same Postgres profile (https://pgsandbox.lvtd.dev/docs/mcp-tools/).

## Why stale sandboxes happen in day-to-day agent workflows

There are three frequent paths where stale resources appear:

- interrupted sessions never run `delete_database`,
- low-priority tasks intentionally leave a timeout buffer for manual inspection,
- profile or workflow failures during migration/clone paths.

These are not safety failures by themselves; they are process failures when no recovery loop runs before the next task starts. That is why `cleanup_expired` is part of production-grade hygiene even on local-first tooling.

The practical loop for coding agents is:

1. create sandboxes with explicit TTL where possible,
2. run proof jobs in the sandbox,
3. delete explicitly when done,
4. run cleanup for interrupted or expired state.

## Use `cleanup_expired` correctly (MCP flow)

### 1) Start with `dryRun`

Use `dryRun` before deletion so your output is a list of `selected` resources and you can compare the response to your expected owner/task scope.

Example:

```json
{
  "tool": "cleanup_expired",
  "arguments": {
    "owner": "agent-session",
    "dryRun": true
  }
}
```

If you pass only one selector, it follows default profile behavior. If multiple profiles are present in your deployment, set your scope explicitly through owner/labels before forcing deletion.

### 2) Filter with `owner` and `labels`

Both filters are additive:

- `owner`: exact owner match,
- `labels`: all provided key-value pairs must match.

Both can be combined. That enables safe workflows like "cleanup only `agent-session` resources with `repo=pgsandbox` labels" while leaving human-managed resources untouched.

This is especially important if multiple agents share a profile and each session writes a different owner/label namespace.

### 3) Run scoped cleanup and read failures

A non-dry run removes rows and returns:

- `deleted`: database ids removed,
- `failures`: deletion or profile-level issues,
- `remainingProfiles` when not cleaning all versions.

When failures are present, treat them as structured operational signals and run targeted follow-up cleanup for the remaining profile set.

From the contract:

- all-version cleanup can continue after one profile fails,
- failures report `category: "profile_unavailable"` with safe messages,
- and profile failures are separate from selected/deleted resources for the successful profile runs.

That is designed to prevent one bad profile from masking others.

## Choose between local, scoped, and all-version cleanup

The tool supports three typical modes:

- **Default profile mode**: safe for day-to-day local cleanup.
- **`profile` mode**: explicit profile for known scope.
- **`postgresVersion: "*"` + `includeAllVersions: true`**: all-version cleanup where needed.

For one-off maintenance on shared operator machines, start narrow first (`owner` + `labels`), then widen only if your first pass intentionally needs cross-version scope.

The agent workflow docs show the same pattern: list by owner first, then call `cleanup_expired` with `includeAllVersions: true` as a separate step when you need the broad view (https://pgsandbox.lvtd.dev/docs/agent-workflows/).

If you are deciding whether a stale resource should go through PGSandbox metadata or a human-run SQL cleanup, use the [cleanup_expired vs manual Postgres cleanup comparison](/blog/cleanup-expired-vs-manual-postgres-cleanup/) before widening scope.

## Recommended operational patterns

### Pattern A: Routine cleanup after agent sessions

Use this for teams that run many short tasks each day.

```bash
pgsandbox list-databases --owner "agent-session"
pgsandbox cleanup-expired --owner "agent-session" --dry-run
pgsandbox cleanup-expired --owner "agent-session"
```

### Pattern B: Task-labeled cleanup to avoid cross-talk

Tag each sandbox with `owner` and at least one stable label, then cleanup by label during handoffs.

- `owner`: `agent-session`
- `labels`: `{"repo":"pgsandbox","kind":"agent-workflow"}`

This keeps a shared profile usable while preventing accidental cleanup overlap.

### Pattern C: Cross-version stale sweep

Use when your local machine runs multiple local majors for migration checks.

```bash
pgsandbox cleanup-expired --postgres-version "*" --dry-run
pgsandbox cleanup-expired --postgres-version "*"
```

Then follow `remainingProfiles` output and repeat for any skipped profile.

## What to expect in logs and events

Each cleanup action is visible through metadata and event trails:

- `pgsandbox_databases` holds ownership, profile, purpose, labels, and timestamps,
- `pgsandbox_events` records cleanup activity with event type and details.

That pairing gives you both state and audit context for what changed and when.

Because PGSandbox runs on `create`/`delete` metadata, a cleanup action does not depend on guessing from names. It can show whether a database id came from a tracked sandbox or from an unowned resource.

## Safe recovery when cleanup is blocked

Cleanup can fail for transient connectivity and for profile mismatch. `failures` are part of expected behavior, not a hard error. Treat them as a control path:

- rerun with a narrower filter and retry,
- check profile health first,
- run `doctor` when profile-level availability is the blocker,
- verify whether `remainingProfiles` indicates the same cleanup should be rerun with another scope.

Manual cleanup should remain a fallback and a logged, scoped operation only when tooling is unavailable. If you do it manually, keep the same principle:

- **only the right owner/superuser can drop a database** (PostgreSQL rule),
- **do not issue drop from unscoped context**,
- **drop is not inside a transaction block**.

The PostgreSQL reference states `DROP DATABASE` can only be executed by the database owner or a superuser, and the command cannot run inside a transaction block (https://www.postgresql.org/docs/current/sql-dropdatabase.html).

Relying on manual SQL outside metadata is exactly how stale-state debt gets created, so use it only for emergency unblocking after confirming scope.

## How cleanup appears in PR-ready proof

A lightweight cleanup block in a PR note keeps the process reviewable:

```text
Cleanup run:
- command: cleanup_expired
- scope: profile=local-pg18, owner=agent-session
- dryRunBeforeDelete: true
- selectedCount: 3
- deletedCount: 3
- failures: 0
- remainingProfiles: none
```

If failures are present, include the unresolved profile names and retry command in follow-up notes before merge.

## Common mistakes and fixes

### Mistake: only deleting manually in `delete_database`

`delete_database` is right for a known sandbox, but expired resources from crashes and timeouts need batch cleanup. Add periodic cleanup to your standard operating cadence.

### Mistake: running destructive cleanup without `dryRun`

That turns cleanup into guesswork. `dryRun` is not optional for shared profiles or label-based cleanup.

### Mistake: filtering by one dimension only

Owner-only cleanup can still hit unrelated resources if owners are reused. Pair owner with labels where team workflows can share an owner value.

### Mistake: never checking `remainingProfiles`

If the cleanup response shows unfinished profile state, stop and run a scoped follow-up instead of assuming system-wide success.

### Mistake: skipping TTL setup when creating task databases

A database with no TTL cannot be selected as stale. Set TTL intentionally for routine work unless the workflow has a stronger explicit retention reason.

## Related pages you should check first

- [PGSandbox MCP Tool Contract](https://pgsandbox.lvtd.dev/docs/mcp-tools/) for all cleanup inputs and output fields.
- [PGSandbox Architecture Notes](https://pgsandbox.lvtd.dev/docs/architecture/) for metadata and resource lifecycle details.
- [PGSandbox Agent Workflows](https://pgsandbox.lvtd.dev/docs/agent-workflows/) for practical cleanup recipes.
- [PostgreSQL `DROP DATABASE` reference](https://www.postgresql.org/docs/current/sql-dropdatabase.html) for owner/superuser requirements when manual fallback is unavoidable.

## FAQ

### Can I run `cleanup_expired` while tasks are still running?

Avoid that for any active test environment. Use owner/label filtering and task ownership so you are deleting only tasks that are already stale.

### Can I call `cleanup_expired` without `dryRun` if I am in a hurry?

You can, but for shared profiles this is a high-risk shortcut. Use `dryRun` as default and reserve immediate destructive cleanup for cases where scope is trivial and reviewed.

### What if cleanup returns profile-level failures?

That is usually a target profile availability issue. Re-run in a narrower scope, run `doctor` for profile health, then execute a second scoped cleanup pass.

### Do I still need `delete_database` in addition to cleanup?

Yes. `delete_database` is best for a known sandbox. `cleanup_expired` is best for stale/expired groups and recovery of interrupted workflows.
