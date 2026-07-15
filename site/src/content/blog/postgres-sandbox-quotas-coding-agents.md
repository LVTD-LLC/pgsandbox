---
title: "Postgres Sandbox Quotas for Coding Agents"
excerpt: "Set per-owner Postgres sandbox quotas that contain abandoned agent work without confusing database count, connection limits, TTLs, and cleanup."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-15"
updatedAt: "2026-07-15T06:00:00Z"
tags: ["Postgres", "MCP", "database sandbox", "quotas", "coding agents"]
category: "Engineering"
metaTitle: "Postgres Sandbox Quotas for Coding Agents"
metaDescription: "Configure per-owner Postgres sandbox quotas for coding agents, choose a practical cap, and recover safely when an owner reaches the limit."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/postgres-sandbox-quotas-coding-agents/"
heroImageUrl: ""
featured: false
sortOrder: 133
---
A Postgres sandbox quota should cap how many live task databases one coding-agent owner can hold on a profile. It should not count SQL queries or replace PostgreSQL connection limits. In PGSandbox, set `maxActiveDatabasesPerOwner`, require a stable non-empty `owner` on every create, delete successful sandboxes immediately, and use TTL cleanup for interrupted work.

That distinction matters on a shared developer machine. A single agent loop can create databases faster than a human notices abandoned work, especially when retries start fresh tasks. A quota contains the count before the profile fills with tracked databases and login roles. It does not make those databases cheaper, shorten their lifetime, or limit how many sessions connect to them.

The operating model in this guide is:

> **active sandbox budget = per-owner quota + short TTL + delete on success + scoped cleanup**

Each control covers a different failure mode. Treating any one of them as the whole policy leaves a gap.

## Quick policy for Postgres sandbox quotas

Use this starting policy for a shared local or private profile:

1. Assign a stable owner such as `codex:repo-a`, `claude:repo-a`, or `ci:repo-a` to every create request.
2. Start with a quota of two active sandboxes per owner for serial agent work, or three when one active task and two bounded investigations may overlap.
3. Keep routine TTLs short enough that interrupted work becomes cleanup-eligible the same day.
4. Call `delete_database` as soon as proof is captured.
5. When creation reaches the cap, list that exact owner's databases before deleting or retrying anything.

The suggested values are policy defaults, not measured capacity claims. Raise them only when observed concurrency requires it and the Postgres host has room for the extra databases, roles, connections, and disk use.

## What does a PGSandbox owner quota count?

PGSandbox counts active metadata rows for an exact owner string inside the selected profile. The current implementation checks four conditions before creating the next sandbox:

- `profile_name` equals the resolved profile;
- `owner` exactly equals the owner in the create request;
- `deleted_at` is null;
- `expires_at` is later than the current time.

If the count is greater than or equal to `maxActiveDatabasesPerOwner`, creation stops with an error before PGSandbox creates the new role and database. You can inspect the query and enforcement path in [`rust-src/postgres.rs`](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/main/rust-src/postgres.rs) and the profile setting in [`rust-src/config.rs`](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/main/rust-src/config.rs).

This accounting boundary has several practical consequences.

First, the quota is **per profile**. An owner with two active sandboxes on `local-pg17` and one on `local-pg18` has separate counts. That is useful when profiles represent different PostgreSQL versions or operational policies, but it means a profile quota is not a machine-wide total.

Second, expired rows do not consume quota slots, even if cleanup has not removed the underlying database and role yet. Expiry and deletion remain different lifecycle states. The [sandbox TTL guide](/blog/postgres-sandbox-ttl-values/) explains why a deadline makes a sandbox eligible for cleanup rather than guaranteeing immediate removal.

Third, owner identity is the key. PGSandbox skips the owner-quota check when `owner` is missing or blank. A quota cannot contain anonymous work. On a shared profile, treat owner as required workflow input even though the tool schema allows it to be omitted.

## Postgres sandbox quotas are not connection limits

There are three different limits that operators often collapse into one:

| Control | Counts | Scope | Best use |
| --- | --- | --- | --- |
| PGSandbox `maxActiveDatabasesPerOwner` | Active tracked sandbox databases | Exact owner within one PGSandbox profile | Prevent one agent identity from accumulating task databases |
| PostgreSQL role `CONNECTION LIMIT` | Concurrent normal connections for one login role | One PostgreSQL role | Bound session concurrency for that role |
| PostgreSQL `max_connections` | Concurrent server connections | PostgreSQL cluster | Set total server connection capacity |

PostgreSQL documents `max_connections` as the server's concurrent connection ceiling and notes that some resource allocations grow with it ([PostgreSQL connection settings](https://www.postgresql.org/docs/current/runtime-config-connection.html)). PostgreSQL also supports `CONNECTION LIMIT` on a login role; `-1` means no role-specific limit ([PostgreSQL `CREATE ROLE`](https://www.postgresql.org/docs/current/sql-createrole.html)).

Neither setting answers, "How many disposable databases may this coding agent leave active?" A sandbox database may have zero connections while waiting for review, or several connections during a test. Database lifecycle count belongs in the lifecycle layer, while connection capacity belongs in PostgreSQL.

That separation follows a broader MCP security principle: minimize the server's scope and privileges instead of relying on instructions alone. The MCP security guidance recommends restricted access and scope minimization for servers that can act on local resources ([MCP security best practices](https://modelcontextprotocol.io/docs/tutorials/security/security_best_practices)). A per-owner lifecycle quota is one narrow policy boundary around a destructive-capable database tool surface.

## Step 1: define an owner identity that survives retries

An owner should identify the budget holder, not a random invocation. If every retry invents a new owner, every retry gets a fresh quota.

Use a stable scheme such as:

```text
<agent-or-runner>:<repository-or-team>
```

Examples:

```text
codex:billing-api
claude:billing-api
ci:billing-api
```

Then put volatile details in labels:

```json
{
  "owner": "codex:billing-api",
  "labels": {
    "repo": "billing-api",
    "task": "migration-184",
    "workflow": "agent-pr",
    "run": "2026-07-15T0600Z"
  }
}
```

This gives the quota a stable subject while preserving task-level traceability. The [owner and label policy](/blog/owner-label-policy-shared-pgsandbox-profiles/) covers naming, cleanup filters, and low-cardinality labels in more detail.

Avoid owners such as a timestamp, UUID, or model message id. Those values are excellent run labels and poor quota identities because no two creates share the same budget.

## Step 2: choose a cap from real overlap

Choose the smallest cap that covers intentional concurrent database work. Count tasks that genuinely need independent live state at the same time, then add at most one recovery slot.

| Workflow shape | Starting cap | Reasoning |
| --- | ---: | --- |
| One agent, serial SQL or migration proof | 2 | One active task plus one bounded recovery or comparison sandbox |
| One agent with frequent before/after comparisons | 3 | Baseline, candidate, and one recovery sandbox |
| Shared CI identity with controlled parallel jobs | Match job concurrency, then test | Each job may require a sandbox, but one shared owner makes the quota an intentional queue boundary |
| Multiple independent agents | Separate stable owners, usually 2-3 each | Prevent one agent's retries from consuming another agent's budget |

Do not derive the cap from `max_connections`. A database count of three does not imply three connections, and three databases can hold very different amounts of data. Check disk, expected clone size, test concurrency, and profile health separately.

For local serial work, a cap of two is intentionally restrictive. If the second slot is routinely occupied by legitimate work, investigate whether proof can be captured and the first sandbox deleted sooner. Raise the cap when overlap is necessary, not because cleanup is inconvenient.

## Step 3: configure the quota on the right profile

For single-profile environment configuration, set:

```bash
export PGSANDBOX_MAX_ACTIVE_DATABASES_PER_OWNER=2
```

The environment variable is optional. If it is unset, the per-owner active sandbox count is unlimited. The repository's [configuration reference](https://github.com/LVTD-LLC/pgsandbox-mcp#environment-variables) documents the same setting alongside TTL controls.

For JSON multi-profile configuration, set the camel-case field on each profile that needs a cap:

```json
{
  "defaultProfile": "local-pg17",
  "profiles": [
    {
      "name": "local-pg17",
      "adminUrl": "postgres://postgres:REDACTED@localhost:6543/postgres",
      "postgresVersion": "17",
      "defaultTtlMinutes": 90,
      "maxTtlMinutes": 240,
      "maxActiveDatabasesPerOwner": 2
    },
    {
      "name": "local-pg18",
      "adminUrl": "postgres://postgres:REDACTED@localhost:6544/postgres",
      "postgresVersion": "18",
      "defaultTtlMinutes": 90,
      "maxTtlMinutes": 240,
      "maxActiveDatabasesPerOwner": 2
    }
  ]
}
```

Use a real secret through your local secret path; do not commit an admin URL. The redacted values above are placeholders.

Run `doctor` after configuration. PGSandbox includes the resolved `maxActiveDatabasesPerOwner` policy in profile diagnostics. This confirms that the process loaded the intended config before an agent begins creating databases.

## Step 4: make create requests quota-accountable

Every create or clone request should carry the stable owner. A normal create can look like:

```json
{
  "nameHint": "migration-184",
  "ttlMinutes": 90,
  "owner": "codex:billing-api",
  "labels": {
    "repo": "billing-api",
    "task": "migration-184",
    "workflow": "agent-pr"
  }
}
```

The [MCP tool contract](/docs/mcp-tools/) describes `create_database`, `clone_database`, `list_databases`, `delete_database`, and `cleanup_expired`. Keep lifecycle operations on that narrow tool surface. Run application SQL through the returned sandbox role rather than the admin profile.

Add one policy sentence to the agent's setup instructions:

```text
Always pass owner=<stable owner> when creating or cloning a sandbox. If quota
is reached, list that owner's active databases on the same profile. Never
change owner or profile merely to bypass the cap.
```

That last sentence matters. A capable agent may interpret a failed create as a reason to vary inputs. The quota error is an operational branch, not a naming problem.

## Step 5: recover when an owner reaches the quota

When creation reports that the owner meets `maxActiveDatabasesPerOwner`, do not retry blindly. Use this recovery sequence:

1. Call `list_databases` with the same profile and owner.
2. Identify sandboxes whose proof has already been captured.
3. Delete completed work by `databaseId` with `delete_database`.
4. Preview expired cleanup with `cleanup_expired`, `dryRun: true`, and the same owner.
5. Run the scoped cleanup only when the preview matches the intended resources.
6. Retry the original create with the original owner and profile.

The [cleanup_expired guide](/blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/) shows the dry-run and filter flow. For resources outside PGSandbox metadata, stop and hand cleanup to a database operator rather than guessing from a name prefix.

Do not delete the oldest sandbox automatically. Age is not proof that work is disposable. Check the task label, retention reason, and evidence state first.

## Test the quota before giving it to agents

Run a small acceptance test against a disposable profile or a local profile with no important sandboxes. A policy that exists only in a config file is easy to misread.

1. Set `maxActiveDatabasesPerOwner` to `2` and restart PGSandbox with that profile.
2. Run `doctor` and confirm the profile reports the configured value.
3. Create two databases with the same stable owner and short TTLs.
4. Attempt a third create with that owner. It should fail before returning a new sandbox.
5. List databases for the owner and confirm the two active metadata rows.
6. Delete one database by `databaseId`, then repeat the third create. It should succeed.
7. Repeat once without an owner in a disposable test environment so the team sees the current bypass behavior rather than discovering it during an incident.

Record only database ids, profile names, owner policy, timestamps, and outcomes in the test evidence. Do not paste admin URLs or returned sandbox credentials into a PR or ticket.

Also test profile separation when agents use multiple PostgreSQL majors. Fill the quota on one profile, then confirm that the same owner has an independent count on the other profile. This verifies the actual contract and prevents a later operator from assuming the setting is global.

The acceptance test should end by deleting every created sandbox. If a deletion fails, use the metadata-backed recovery flow and keep the failure details; do not make the quota test an excuse for manual prefix-based database removal.

## Know the current concurrency boundary

The present implementation performs a count check and then creates the role and database. It does not reserve a quota slot with a database lock or atomic counter. Two truly concurrent create requests for the same owner can both observe an available slot before either inserts its metadata row.

That means `maxActiveDatabasesPerOwner` is a practical guard for normal agent workflows, not a strict admission-control primitive under adversarial concurrency. If many parallel jobs share one owner, serialize sandbox creation in the runner or give each controlled worker a stable owner with its own cap. A hosted multi-tenant service would need stronger transactional admission control.

This limitation is also why the quota should not be described as a billing or security boundary. It reduces accidental accumulation on a local or private profile. It does not replace authentication, tenant isolation, disk monitoring, PostgreSQL connection policy, or OS-level resource controls.

## Common mistakes with Postgres sandbox quotas

### Setting a cap but omitting owner

Blank or missing owners skip quota enforcement. Make owner mandatory in the workflow contract.

### Generating one owner per task

Unique task owners make every task its own unlimited budget. Keep the owner stable and put task identity in labels.

### Raising the cap instead of deleting completed work

A higher cap hides slow cleanup. Capture portable proof, delete on success, and reserve live sandboxes for work that still needs them.

### Treating expiry as deletion

Expired sandboxes no longer count against the current quota query, but their database and role may remain until cleanup succeeds. Inspect with `includeExpired` and run scoped cleanup.

### Treating a lifecycle quota as a capacity plan

The cap does not measure database size, query cost, or connection count. Monitor those separately at the PostgreSQL and host layers.

## A five-part policy worth copying

Use this compact contract for shared coding-agent profiles:

```text
Every sandbox create or clone must use a stable owner formatted as
<agent-or-runner>:<repo-or-team>. Set maxActiveDatabasesPerOwner to the smallest
value that covers measured overlap, usually 2 for serial work or 3 for bounded
comparison work. Use task-specific labels and a short TTL. Delete on successful
proof. On quota failure, list the same owner's sandboxes, delete completed work,
preview owner-scoped expired cleanup, and retry without changing identity.
Serialize create operations when strict parallel admission matters.
```

This policy is useful without a search engine or an AI assistant. A human can inspect it, an agent can execute it, and an operator can tell which control failed.

## FAQ

### What is the default per-owner quota in PGSandbox?

There is no default cap. `maxActiveDatabasesPerOwner` and `PGSANDBOX_MAX_ACTIVE_DATABASES_PER_OWNER` are optional; when unset, active sandbox count per owner is unlimited. Configure the value explicitly on shared profiles.

### Do expired sandboxes count against the quota?

No. The current PGSandbox quota query counts metadata rows whose expiry is still in the future and whose deletion marker is null. Expired resources can still exist until `cleanup_expired` or explicit deletion removes them.

### Can an ownerless sandbox bypass the quota?

Yes. PGSandbox currently skips per-owner quota enforcement when the create request has no owner or a blank owner. Shared-profile workflows should require a stable non-empty owner on every create and clone.

### Is a sandbox quota the same as PostgreSQL CONNECTION LIMIT?

No. A sandbox quota counts active tracked databases for one PGSandbox owner and profile. PostgreSQL `CONNECTION LIMIT` counts concurrent normal connections for a login role. Use both when both lifecycle accumulation and connection concurrency need controls.

### What should an agent do after a quota error?

List databases for the same owner and profile, delete completed work by `databaseId`, preview owner-scoped expired cleanup, and retry with the same identity. Do not evade the policy by changing owner or profile.

## Related pages

- [PGSandbox architecture](/docs/architecture/)
- [PGSandbox MCP tool contract](/docs/mcp-tools/)
- [Choosing Postgres sandbox TTLs](/blog/postgres-sandbox-ttl-values/)
- [Owner and label policy for shared profiles](/blog/owner-label-policy-shared-pgsandbox-profiles/)
- [Postgres MCP server safety checklist](/blog/postgres-mcp-server-safety-checklist/)

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Article",
      "headline": "Postgres Sandbox Quotas for Coding Agents",
      "datePublished": "2026-07-15",
      "dateModified": "2026-07-15",
      "author": {"@type": "Organization", "name": "PGSandbox Team"},
      "mainEntityOfPage": "https://pgsandbox-mcp.lvtd.dev/blog/postgres-sandbox-quotas-coding-agents/"
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "Postgres Sandbox Quotas for Coding Agents", "item": "https://pgsandbox-mcp.lvtd.dev/blog/postgres-sandbox-quotas-coding-agents/"}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {
          "@type": "Question",
          "name": "What is the default per-owner quota in PGSandbox?",
          "acceptedAnswer": {"@type": "Answer", "text": "There is no default cap. The per-owner active sandbox quota is optional and remains unlimited until an operator configures it."}
        },
        {
          "@type": "Question",
          "name": "Do expired sandboxes count against the quota?",
          "acceptedAnswer": {"@type": "Answer", "text": "No. The current quota query counts undeleted metadata rows whose expiry is still in the future. Expired resources can remain until cleanup or deletion succeeds."}
        },
        {
          "@type": "Question",
          "name": "Can an ownerless sandbox bypass the quota?",
          "acceptedAnswer": {"@type": "Answer", "text": "Yes. PGSandbox currently skips per-owner quota enforcement when owner is missing or blank, so shared-profile workflows should require a stable non-empty owner."}
        },
        {
          "@type": "Question",
          "name": "Is a sandbox quota the same as PostgreSQL CONNECTION LIMIT?",
          "acceptedAnswer": {"@type": "Answer", "text": "No. A sandbox quota counts active tracked databases for one owner and profile, while PostgreSQL CONNECTION LIMIT counts concurrent normal connections for a login role."}
        },
        {
          "@type": "Question",
          "name": "What should an agent do after a quota error?",
          "acceptedAnswer": {"@type": "Answer", "text": "List databases for the same owner and profile, delete completed work by databaseId, preview owner-scoped expired cleanup, and retry with the same identity."}
        }
      ]
    }
  ]
}
</script>
