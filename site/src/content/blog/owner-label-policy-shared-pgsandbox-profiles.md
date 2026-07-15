---
title: "Owner and Label Policy for Shared PGSandbox Profiles"
excerpt: "Design a stable owner and label policy for PGSandbox cleanup so shared agent profiles stay auditable, scoped, and easy to recover."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-13"
updatedAt: "2026-07-13T06:00:00Z"
tags: ["Postgres", "MCP", "cleanup", "database sandbox", "agent safety"]
category: "Engineering"
metaTitle: "PGSandbox Owner and Label Policy"
metaDescription: "Design owner and label policy for shared PGSandbox profiles: cleanup scopes, TTL hygiene, audit fields, label taxonomy, and PR proof notes."
canonicalUrl: "https://pgsandbox.lvtd.dev/blog/owner-label-policy-shared-pgsandbox-profiles/"
heroImageUrl: ""
featured: false
sortOrder: 133
---
Use a stable `owner` for the actor or automation lane, then use labels for the repo, branch, task, workflow, and retention reason. That is the safest owner and label policy for shared PGSandbox profiles because `owner` is an exact-match cleanup boundary, while `labels` let you narrow cleanup to the specific work that produced the sandbox.

For shared coding-agent machines, do not treat labels as decoration. PGSandbox stores owner and labels in lifecycle metadata, exposes them through `list_databases`, and uses them in `cleanup_expired` filtering. A cleanup run should be able to answer: which workflow created this sandbox, why is it expired, which repo/task owns it, and what can be deleted without crossing into another agent's work.

The information-gain point is the policy itself: design labels as a cleanup contract, not as tags for later search. The contract should be small enough that every agent can apply it, but strict enough that cleanup never falls back to guessing from database names.

## The policy in one table

| Field | Recommended value | Why it matters |
| --- | --- | --- |
| `owner` | Stable actor or lane: `codex`, `forge`, `ci`, `human-rk` | Exact-match filter for `list_databases` and `cleanup_expired`. |
| `labels.repo` | Repository slug: `pgsandbox-mcp` | Prevents cross-repo cleanup on shared local profiles. |
| `labels.task` | Issue, PR, session, or cron id | Gives cleanup and review notes a durable task handle. |
| `labels.branch` | Short branch name when useful | Helps humans recover interrupted branch work. |
| `labels.workflow` | `migration`, `bug-repro`, `seed`, `cleanup-test`, `docs-proof` | Explains why the sandbox exists. |
| `labels.retention` | `delete-on-success`, `review-until-ttl`, `manual-hold` | Tells cleanup whether expiry is expected or suspicious. |
| `ttlMinutes` | Short positive TTL by default | Gives stale sandboxes a mechanical cleanup path. |

That is enough for most agent workflows. Add labels only when they change cleanup behavior or review clarity. A label that nobody uses in selection, audit, or recovery is just metadata noise.

## Why owner alone is too broad

PGSandbox's MCP tool contract documents `owner` as an optional agent/session identifier on creation tools and as an owner filter on `list_databases` and `cleanup_expired` (https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/). When supplied to cleanup, the owner must exactly match the stored owner.

That exact match is useful, but it is not enough on machines where one owner value runs several repos or task types. If every sandbox uses `owner: "agent-session"`, owner-only cleanup can select expired sandboxes from unrelated work. That may be acceptable on a single-purpose laptop. It is a weak boundary for shared agent hosts, cron jobs, or long-running operator machines.

Use `owner` to answer "who or what owns this lane?" Use labels to answer "which work should this cleanup run touch?"

```json
{
  "tool": "create_database",
  "arguments": {
    "nameHint": "billing migration proof",
    "ttlMinutes": 45,
    "owner": "codex",
    "labels": {
      "repo": "pgsandbox-mcp",
      "task": "pr-184",
      "workflow": "migration",
      "retention": "delete-on-success"
    }
  }
}
```

This is more useful than a clever `nameHint`. The database name is generated. The metadata is the durable selection surface.

## How PGSandbox applies owner and label filters

PGSandbox records sandbox lifecycle metadata in `pgsandbox_databases`, including database id, profile name, database name, role name, owner, purpose, labels, timestamps, and deletion state (https://pgsandbox-mcp.lvtd.dev/docs/architecture/).

The `cleanup_expired` contract is deliberately narrow:

- `owner` selects expired sandboxes whose stored owner exactly matches the provided value.
- `labels` selects expired sandboxes whose stored labels contain every provided key/value pair.
- When both are supplied, both filters must match.
- Sandboxes may have additional labels and still match the cleanup filter.

That containment rule matters. If you run cleanup with:

```json
{
  "tool": "cleanup_expired",
  "arguments": {
    "owner": "codex",
    "labels": {
      "repo": "pgsandbox-mcp",
      "workflow": "migration"
    },
    "dryRun": true
  }
}
```

then a sandbox with `repo=pgsandbox-mcp`, `workflow=migration`, `task=pr-184`, and `retention=review-until-ttl` still matches. A sandbox from another repo does not.

The implementation follows the same shape: list queries can filter by owner, and cleanup builds a selection query with owner equality plus JSONB label containment. PostgreSQL documents `jsonb` containment with the `@>` operator, where the left JSONB value contains the right JSONB value at the top level (https://www.postgresql.org/docs/current/functions-json.html). PGSandbox uses that behavior to make a small cleanup filter match richer stored metadata without requiring every label to be repeated.

## A practical owner taxonomy

Keep owners stable, few, and operational. Do not put every task id into `owner`; put task ids in labels.

Good owner values:

| Owner | Use it for |
| --- | --- |
| `codex` | Local or hosted Codex coding-agent runs. |
| `forge` | A named internal engineering agent lane. |
| `ci` | Automated CI or pre-merge validation. |
| `human-rk` | A human operator's personal sandbox lane. |
| `cron-seo` | Scheduled automation that may need database proof. |

Poor owner values:

| Owner | Problem |
| --- | --- |
| `agent-session` everywhere | Too broad once several workflows share a profile. |
| Full branch names with slashes | They change often and belong in labels. |
| Raw email addresses | Unnecessary personal data in local metadata. |
| Secrets, database URLs, or hostnames | They should never enter metadata or PR notes. |
| One-off random ids only | Hard for humans to group during recovery. |

The owner should be stable enough to support per-owner quotas. PGSandbox supports optional per-owner active sandbox quotas in profile config, so a high-cardinality owner value can make quota behavior less useful. The [Postgres sandbox quota guide](/blog/postgres-sandbox-quotas-coding-agents/) shows how exact owner matching, TTL state, and profile scope determine the active count.

## A label taxonomy that survives cleanup

Use labels that answer cleanup questions. This set works for most teams:

| Label key | Example | Required? | Cleanup value |
| --- | --- | --- | --- |
| `repo` | `pgsandbox-mcp` | Yes for shared machines | Prevents cross-repo deletion. |
| `task` | `PGS-043`, `pr-184`, `cron-20260713` | Yes when available | Ties cleanup to work. |
| `workflow` | `migration`, `clone`, `seed`, `bug-repro` | Yes | Explains why the sandbox exists. |
| `branch` | `fix-billing-status` | Optional | Helps interrupted branch recovery. |
| `agent` | `codex`, `forge` | Optional if owner already says it | Useful when owner is a lane. |
| `retention` | `delete-on-success`, `review-until-ttl`, `manual-hold` | Recommended | Explains expected cleanup timing. |

Keep values short and boring. Label values should be safe to paste into a PR note. They should not contain customer data, connection strings, local filesystem paths, or raw SQL.

## Use labels differently for different workflows

For migration testing, the task boundary is usually a PR or issue:

```json
{
  "owner": "codex",
  "labels": {
    "repo": "pgsandbox-mcp",
    "task": "pr-184",
    "workflow": "migration",
    "retention": "review-until-ttl"
  }
}
```

For a bug reproduction, keep the branch and bug handle:

```json
{
  "owner": "forge",
  "labels": {
    "repo": "billing-api",
    "task": "bug-912",
    "branch": "fix-empty-ledger-state",
    "workflow": "bug-repro",
    "retention": "delete-on-success"
  }
}
```

For CI, use stable labels that do not depend on a local agent session:

```json
{
  "owner": "ci",
  "labels": {
    "repo": "pgsandbox-mcp",
    "task": "run-88421",
    "workflow": "pre-merge",
    "retention": "delete-on-success"
  }
}
```

The goal is not a universal naming standard. The goal is that every cleanup run can filter narrowly without inventing new logic at cleanup time.

## The cleanup flow for shared profiles

Start with inventory. `list_databases` returns active database metadata without full secrets, including database id, database name, role name, profile, creation and expiration timestamps, and TTL state. It excludes expired sandboxes by default unless `includeExpired` is true (https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/).

For a shared profile, use this cleanup sequence:

1. List by owner.
2. If needed, list with `includeExpired`.
3. Run `cleanup_expired` with `dryRun`.
4. Confirm the selected resources match owner and labels.
5. Run the same cleanup without `dryRun`.
6. Record selected, deleted, failures, and remaining profiles in the PR or task note.

Example:

```json
{
  "tool": "list_databases",
  "arguments": {
    "owner": "codex",
    "includeExpired": true
  }
}
```

Then:

```json
{
  "tool": "cleanup_expired",
  "arguments": {
    "owner": "codex",
    "labels": {
      "repo": "pgsandbox-mcp",
      "task": "pr-184"
    },
    "dryRun": true
  }
}
```

Only run destructive cleanup after the dry-run output matches the intended work. This follows the same pattern as the [cleanup_expired stale resource guide](/blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/).

## Cross-version cleanup needs stricter labels

PGSandbox can list or clean across configured profiles and running managed-local version profiles with `includeAllVersions` or `postgresVersion: "*"`. The tool contract says all-version cleanup continues across profiles and reports profile-level failures separately (https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/).

That broad scope is useful when agents test multiple Postgres versions, but it raises the cost of weak labels. Do not run all-version cleanup with only `owner` unless the owner is already unique to the task.

Prefer:

```json
{
  "tool": "cleanup_expired",
  "arguments": {
    "includeAllVersions": true,
    "owner": "codex",
    "labels": {
      "repo": "pgsandbox-mcp",
      "task": "pr-184"
    },
    "dryRun": true
  }
}
```

Then check `profiles`, `remainingProfiles`, and `failures`. A cross-version cleanup note that ignores failed profiles is incomplete.

## What not to put in labels

Treat labels as operational metadata that may show up in local logs, tool output, PR proof, or review notes. That makes some values bad label candidates:

- full database URLs,
- passwords, tokens, or API keys,
- customer identifiers,
- full local filesystem paths,
- raw SQL,
- large JSON payloads,
- vague flags such as `important=true` with no cleanup meaning.

If a value would be unsafe in a GitHub PR comment, do not put it in `labels`.

## When manual cleanup still enters the picture

Owner and label policy does not replace PostgreSQL operator rules. It keeps PGSandbox cleanup scoped to resources PGSandbox created.

If a database is outside PGSandbox metadata, PGSandbox should not delete it by name. Use manual Postgres cleanup only as a human-reviewed fallback. PostgreSQL documents that `DROP DATABASE` cannot run while connected to the target database, cannot run inside a transaction block, and removes the database permanently (https://www.postgresql.org/docs/current/sql-dropdatabase.html). Role cleanup can also require `REASSIGN OWNED`, `DROP OWNED`, and dependency checks across databases (https://www.postgresql.org/docs/current/role-removal.html).

That is why label policy matters: it reduces the number of times a human has to reconstruct ownership from names and manual queries.

For the full decision boundary, read the [cleanup_expired vs manual Postgres cleanup comparison](/blog/cleanup-expired-vs-manual-postgres-cleanup/).

## PR-ready proof format

Use a compact proof block. It should be safe to paste into a PR and useful for a reviewer:

```text
PGSandbox cleanup:
- owner: codex
- labels: repo=pgsandbox-mcp, task=pr-184, workflow=migration
- profile scope: local-pg18
- dryRunReviewed: true
- selected: 1
- deleted: 1
- failures: 0
- remainingProfiles: none
```

If cleanup is skipped, say why:

```text
PGSandbox cleanup:
- owner: codex
- labels: repo=pgsandbox-mcp, task=pr-184, workflow=migration
- skipped: sandbox retained until TTL for reviewer inspection
- expiresAt: 2026-07-13T08:30:00Z
```

This is the difference between "the agent cleaned up" and "the reviewer can see the cleanup boundary."

## How to migrate an existing loose policy

Most teams do not start with a perfect taxonomy. They start with a few agents using `owner: "agent-session"` and maybe a task name in `nameHint`. That is normal. Do not rewrite every workflow at once. Move the policy in three passes.

First, standardize the owner. Pick the small owner set you want to support and update the agent setup prompt or repo workflow docs so new sandboxes use it. For example, change generic `agent-session` to `codex`, `forge`, or `ci` depending on who creates the sandbox.

Second, add `labels.repo` and `labels.workflow` everywhere. Those two fields provide the biggest cleanup improvement because they separate unrelated repositories and explain why the sandbox exists. A profile with one owner and three repos becomes much easier to inspect once every sandbox carries `repo`.

Third, add `labels.task` and `labels.retention` in workflows that open PRs or intentionally leave sandboxes behind for review. Those labels turn cleanup from a housekeeping action into review evidence.

During the migration, keep cleanup conservative:

```json
{
  "tool": "cleanup_expired",
  "arguments": {
    "owner": "agent-session",
    "dryRun": true
  }
}
```

If the dry run shows mixed work, do not run the destructive pass by owner alone. Delete known sandboxes by `databaseId`, or wait until the new label policy has covered the active workflows.

## Audit the policy before automating cleanup

Before you put cleanup on a schedule, run a policy audit against active and expired sandboxes:

| Check | Pass condition | Fix |
| --- | --- | --- |
| Owner consistency | Most sandboxes use one of the approved owner values | Update agent prompts and repo workflow examples. |
| Repo label coverage | Shared-profile sandboxes include `labels.repo` | Add repo labels to create/clone/template restore calls. |
| Workflow label coverage | Sandboxes say `migration`, `bug-repro`, `seed`, or another known workflow | Add workflow labels where tasks are created. |
| Retention clarity | Review sandboxes say why they are retained | Add `retention=review-until-ttl` or `manual-hold`. |
| Secret hygiene | Labels contain no URLs, tokens, customer data, or paths | Rotate any leaked secrets and stop writing them to metadata. |
| Cleanup proof | PR notes include selected/deleted/failure counts | Add the proof block to the agent's PR checklist. |

This audit can be manual at first. The important habit is reading metadata before trusting cleanup automation.

## The policy should be part of the agent prompt

If humans have to remember the label policy each time, it will drift. Put the policy where sandboxes are created:

- repo setup docs,
- MCP client instructions,
- agent setup prompts,
- CI scripts,
- task templates,
- PR checklist examples.

For PGSandbox, the [MCP tool contract](/docs/mcp-tools/) already names `owner` and `labels` as creation inputs. The policy layer should sit one level above that: "for this repo, use these values."

Example prompt language:

```text
When creating a PGSandbox database for this repo, set owner=codex and labels.repo=pgsandbox-mcp. Add labels.task when a PR, issue, or cron id exists. Add labels.workflow as migration, bug-repro, seed, or docs-proof. Use retention=delete-on-success unless the sandbox is intentionally kept until TTL for review.
```

That sentence is more valuable than a long cleanup runbook that agents only see after something is stale.

## When to widen cleanup scope

Widen cleanup only when the narrower scope proves incomplete.

Start with a known database id when the task has one. If the id is gone or the agent lost context, list by owner. If owner shows mixed work, add repo and task labels. If the task touched several Postgres versions, widen to all versions only after the dry-run result is narrow enough to review.

The escalation path should look like this:

1. `delete_database` by `databaseId`.
2. `cleanup_expired` by owner + repo + task labels.
3. `cleanup_expired` by owner + repo + workflow labels.
4. `cleanup_expired` with `includeAllVersions` and the same owner/label filters.
5. Manual Postgres cleanup only for resources outside PGSandbox metadata.

This order keeps destructive authority close to the task that created the resource. It also matches the product boundary: PGSandbox is best at deleting metadata-owned sandboxes, not at acting like a general database administrator.

## Common mistakes

### Mistake: using the task id as the owner

That makes owners too high-cardinality and weakens per-owner inventory. Put task ids in `labels.task`.

### Mistake: using labels only on create, not cleanup

Labels earn their keep during selection. If cleanup runs by owner only, the label policy is not actually part of the safety model.

### Mistake: using branch-only cleanup

Branches get renamed and reused. Use branch as supporting context, not as the primary cleanup boundary.

### Mistake: forgetting retention intent

Some sandboxes should be deleted on success. Others should live until TTL for review. A `retention` label keeps that difference visible.

The [Postgres sandbox TTL guide](/blog/postgres-sandbox-ttl-values/) provides a retention-budget method and starting ranges for direct SQL checks, migration validation, repository commands, and human review holds.

### Mistake: making labels too personal

Prefer role or lane identifiers. The cleanup system needs operational ownership, not personal data.

## FAQ

### Is `owner` required for PGSandbox sandboxes?

No. PGSandbox treats `owner` as optional metadata. In shared agent workflows, you should still set it because it gives `list_databases`, cleanup, quotas, and PR proof a stable grouping field.

### Should labels be unique for every sandbox?

Not all of them. `labels.task` can be unique, but `labels.repo` and `labels.workflow` should be reusable so you can run scoped cleanup across a class of work.

### Can labels select active sandboxes?

`cleanup_expired` selects expired resources. Use `list_databases` for inventory, including active resources. Set `includeExpired` when you need to inspect expired-but-not-deleted sandboxes.

### Should I run `includeAllVersions` by default?

No. Start with the default or explicit profile. Use all-version cleanup only when the task intentionally touched multiple Postgres versions, and pair it with owner plus labels.

## Related pages

- [Postgres Sandbox Quotas for Coding Agents](/blog/postgres-sandbox-quotas-coding-agents/)
- [How to Use cleanup_expired for Stale PGSandbox Resources](/blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/)
- [cleanup_expired vs Manual Postgres Cleanup for Agent Sandboxes](/blog/cleanup-expired-vs-manual-postgres-cleanup/)
- [Postgres MCP Server Safety Checklist for Coding Agents](/blog/postgres-mcp-server-safety-checklist/)
- [MCP tool contract](/docs/mcp-tools/)
- [Architecture](/docs/architecture/)

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "FAQPage",
  "mainEntity": [
    {
      "@type": "Question",
      "name": "Is owner required for PGSandbox sandboxes?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "No. PGSandbox treats owner as optional metadata. In shared agent workflows, setting it gives list_databases, cleanup, quotas, and PR proof a stable grouping field."
      }
    },
    {
      "@type": "Question",
      "name": "Should labels be unique for every sandbox?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "Not all labels should be unique. labels.task can be unique, while labels.repo and labels.workflow should be reusable so cleanup can select a class of work."
      }
    },
    {
      "@type": "Question",
      "name": "Can labels select active sandboxes?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "cleanup_expired selects expired resources. Use list_databases for inventory, including active resources, and set includeExpired when inspecting expired-but-not-deleted sandboxes."
      }
    },
    {
      "@type": "Question",
      "name": "Should I run includeAllVersions by default?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "No. Start with the default or explicit profile. Use all-version cleanup only when the task intentionally touched multiple Postgres versions, and pair it with owner plus labels."
      }
    }
  ]
}
</script>
