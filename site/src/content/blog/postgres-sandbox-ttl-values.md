---
title: "Postgres Test Database Cleanup: Choosing Sandbox TTLs"
excerpt: "Choose a Postgres sandbox TTL from task runtime, review time, and recovery margin instead of copying one timeout across every coding-agent workflow."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-14"
updatedAt: "2026-07-14T15:04:00Z"
tags: ["Postgres", "MCP", "database sandbox", "TTL", "cleanup"]
category: "Engineering"
metaTitle: "Postgres Test Database Cleanup: Choosing TTLs"
metaDescription: "Choose Postgres sandbox TTL values by task runtime, review buffer, and recovery margin, then verify expiry and cleanup with PGSandbox."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/postgres-sandbox-ttl-values/"
heroImageUrl: ""
featured: false
sortOrder: 134
---
A useful Postgres sandbox TTL is long enough for the task, review, and one recovery attempt, but short enough that abandoned databases do not become permanent local state. For most coding-agent work, start at 45 minutes for direct SQL checks, 90 minutes for migrations, and 240 minutes when a human needs time to inspect the result. Delete on success; treat TTL as the deadline for cleanup after interruption.

That is the core of reliable Postgres test database cleanup. A single timeout copied across every workflow is easy to configure, but it ignores why a sandbox exists. A schema check and a bug reproduction have different runtimes, review needs, and failure modes.

This guide uses a simple retention budget:

> **sandbox TTL = expected task runtime + review buffer + recovery margin**

The budget is a policy framework, not a benchmark. Measure your own task durations and adjust the ranges. The important part is that each minute has a reason.

## Postgres test database cleanup: quick TTL recommendations

| Workflow | Starting TTL | Why |
| --- | ---: | --- |
| Direct SQL or schema check | 15-45 minutes | The proof is immediate and should be captured in the same session. |
| Migration or seed validation | 45-90 minutes | Allows setup, execution, schema inspection, and one correction. |
| Repository test command | 60-120 minutes | Leaves room for install, migration, test, and diagnostic output. |
| Human review of a failed proof | 120-240 minutes | Keeps the sandbox available through a short handoff without making it long-lived. |
| Multi-version or slow clone investigation | 240-480 minutes | Covers serial work across profiles or a deliberately retained reproduction. |

Do not make 480 minutes the default just because one workflow occasionally needs it. Put the longer TTL on that workflow. Keep the profile default representative of routine work.

PGSandbox's built-in profile defaults are 240 minutes for `defaultTtlMinutes` and 1,440 minutes for `maxTtlMinutes` when a profile does not override them ([source](https://github.com/LVTD-LLC/pgsandbox/blob/main/rust-src/config.rs)). Those values are guardrails, not a claim that every task should live for four hours or may safely live for a day.

## Step 1: measure the task runtime

Start with the time from sandbox creation to the last required proof step. Include operations the agent actually performs:

1. Create or restore the sandbox.
2. Apply migrations or seed data.
3. Run the repository command or SQL.
4. Inspect schema, rows, or an EXPLAIN plan.
5. Record the result for the PR or task.

Do not use the agent's entire session length. If a coding task takes two hours but database work occupies 18 minutes near the end, the sandbox does not automatically need a two-hour TTL. Create it close to the proof step and budget for the database work.

For a repository command, align the TTL with the command's own timeout plus setup and inspection. GitHub Actions, for example, supports `jobs.<job_id>.timeout-minutes` and documents a 360-minute default ([GitHub Actions workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)). That does not mean a sandbox should inherit 360 minutes blindly. It means the database deadline must not expire before the job that uses it can finish.

When there is no timing history, run three representative tasks and record creation, last database action, and deletion times. The longest normal run is a better starting point than intuition.

## Step 2: add a review buffer only when review needs the database

A reviewer often needs the proof, not the live sandbox. A schema diff, bounded query result, migration log, or test output can survive after database deletion. In that case, the review buffer is zero: capture the evidence and call `delete_database`.

Keep the sandbox for review only when the reviewer must query it, inspect an unusual state, or reproduce a failure that is not fully represented in the saved output. Then add a clear buffer:

- 30-60 minutes for a synchronous handoff.
- 120-240 minutes for an expected same-day review.
- Longer only when a named operator owns the hold and the profile permits it.

Add `retention=review-until-ttl` and a task label when you retain a sandbox. The [owner and label policy](/blog/owner-label-policy-shared-pgsandbox-profiles/) explains how stable owner, repo, task, and workflow metadata keep cleanup narrow on shared machines.

A TTL is a poor substitute for a review artifact. If nobody knows why the sandbox remains, a longer timeout only delays the ambiguity.

## Step 3: reserve one recovery attempt

The recovery margin covers a bounded retry after a predictable failure: a migration typo, a missing extension, a stale connection, or a profile that needs a health check. It should not fund indefinite debugging.

A practical rule is to add 25-50% of the expected runtime, capped at one normal retry. A 30-minute migration workflow might use 45 minutes. A 60-minute repository validation might use 90 minutes. If the workflow repeatedly consumes the margin, fix the workflow or raise its explicit TTL based on observed data.

PGSandbox resolves the requested `ttlMinutes`, rejects zero and negative values, rejects values above the profile's `maxTtlMinutes`, and calculates `expiresAt` from the resolved duration. Omitting `ttlMinutes` uses the profile default. The behavior is documented in the [MCP tool contract](/docs/mcp-tools/) and enforced in the repository's lifecycle implementation ([source](https://github.com/LVTD-LLC/pgsandbox/blob/main/rust-src/postgres.rs)).

## Step 4: set the TTL explicitly for exceptional workflows

Use the profile default for the common case. Pass `ttlMinutes` when the workflow has a justified shorter or longer budget.

```json
{
  "tool": "create_database",
  "arguments": {
    "nameHint": "billing migration proof",
    "ttlMinutes": 90,
    "owner": "codex",
    "labels": {
      "repo": "billing-api",
      "task": "pr-184",
      "workflow": "migration",
      "retention": "delete-on-success"
    }
  }
}
```

The same approach applies to `clone_database`, `validate_schema_change`, and `create_sandbox_from_template`, which accept TTL input when they create a fresh sandbox. Follow the existing [agent workflow examples](/docs/agent-workflows/) and keep the value tied to the operation that owns the database.

Do not encode retention policy only in `nameHint`. PGSandbox stores `expiresAt`, owner, purpose, and labels as lifecycle metadata. Structured fields can be listed, filtered, and used for cleanup; a clever database name cannot carry the same contract.

## Step 5: verify Postgres test database cleanup after expiry

TTL does not mean “leave every database until it expires.” Normal completion should still call `delete_database`. TTL is the recovery deadline for crashes, canceled jobs, lost agent context, and review holds.

Use this closeout sequence:

1. Capture the proof needed by the PR or task.
2. Call `delete_database` for the known sandbox.
3. If the session was interrupted, use `list_databases` with `includeExpired` when needed.
4. Preview expired candidates with `cleanup_expired` and `dryRun`.
5. Run the same owner/label-scoped cleanup after the preview matches expectations.

The [cleanup_expired guide](/blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/) covers that recovery path in detail. PGSandbox cleanup selects tracked resources whose `expiresAt` has passed; expiry marks eligibility for cleanup, while deletion removes the database and role.

That distinction matters because PostgreSQL database removal is destructive and authority-sensitive. PostgreSQL documents that database removal requires the owner or a superuser, cannot run while connected to the target, and may fail when connections remain unless `FORCE` can terminate them ([PostgreSQL `dropdb`](https://www.postgresql.org/docs/current/app-dropdb.html), [PostgreSQL `DROP DATABASE`](https://www.postgresql.org/docs/16/sql-dropdatabase.html)). Metadata-backed cleanup gives the operator a narrower selection surface than guessing from database names.

## How to choose the profile default and maximum TTL

Set `defaultTtlMinutes` to cover most routine workflows without an override. Set `maxTtlMinutes` to the longest period the machine's operator is willing to permit for an explicitly retained sandbox.

```json
{
  "defaultProfile": "local-pg18",
  "profiles": [
    {
      "name": "local-pg18",
      "adminUrl": "postgres://<redacted>@127.0.0.1:5432/postgres",
      "defaultTtlMinutes": 90,
      "maxTtlMinutes": 480,
      "maxActiveDatabasesPerOwner": 3
    }
  ]
}
```

Choose the two values separately:

- **Default:** the 75th or 90th percentile of routine task runtime plus a small recovery margin.
- **Maximum:** the longest approved debugging or review hold, not an effectively permanent setting.

Pair retention with a per-owner quota on shared profiles. A short TTL limits age; a quota limits how many active sandboxes one owner can accumulate before those deadlines arrive. These controls solve different problems.

Revisit the policy after a week or a representative batch of tasks. Count sandboxes deleted on success, cleaned after expiry, and still active near their deadline. If routine tasks often expire while active, the budget is too short or sandbox creation happens too early. If most sandboxes wait for TTL despite successful tasks, the closeout workflow is missing explicit deletion.

## Postgres sandbox TTL anti-patterns

### One TTL for every task

This makes short checks live too long or slow workflows expire mid-run. Keep a representative profile default and override real exceptions.

### TTL equal to expected runtime

The first retry consumes time the budget did not include. Add a bounded recovery margin.

### Review buffer without an owner

If a sandbox is retained for review, label the task and retention reason. Otherwise the next cleanup operator cannot distinguish an intentional hold from abandoned state.

### Treating expiry as deletion

An expired database may still exist until cleanup runs. Use `list_databases` to inspect state and `cleanup_expired` to remove eligible tracked resources.

### Using TTL instead of explicit cleanup

Successful workflows should delete their sandbox. Relying only on expiry increases active database count and makes local state harder to reason about.

## A compact policy for coding agents

Give agents a policy they can execute without making up values per run:

```text
For Postgres proof work, choose ttlMinutes as expected database-task runtime
+ review buffer (only if the live database is needed)
+ one recovery attempt. Use 45 minutes for direct SQL checks, 90 minutes for
migration or repo validation, and 240 minutes for an explicit same-day review
hold. Label retained sandboxes with repo, task, workflow, and retention reason.
Always delete on success. Use cleanup_expired dry-run before deletion sweeps.
Never exceed the profile max TTL.
```

That policy turns TTL from a vague safety feature into an operational deadline. The result is a sandbox that lives long enough to prove the task and no longer than the evidence requires.

## FAQ

### What is the default PGSandbox TTL?

PGSandbox uses a built-in default of 240 minutes when a profile does not set `defaultTtlMinutes`. Profiles can override that value. A task can also pass a positive `ttlMinutes` value up to the profile's `maxTtlMinutes`.

### Does a Postgres sandbox disappear exactly when its TTL expires?

No. In PGSandbox, expiry makes a tracked sandbox eligible for `cleanup_expired`. The database and role are removed when explicit deletion or cleanup succeeds. Use `list_databases` with `includeExpired` to inspect expired-but-not-deleted resources.

### Should long-running tests get a longer TTL?

Yes, when measured runtime requires it. Budget for setup, the test, proof capture, and one recovery attempt. Keep the longer value on that workflow instead of increasing the default for every sandbox.

### How long should a sandbox stay for code review?

Keep it only when the reviewer needs the live database. Start with 120-240 minutes for a same-day review, add task and retention labels, and save portable proof such as schema diffs or bounded query results. Delete the sandbox as soon as review no longer needs it.

## Related pages

- [PGSandbox MCP tool contract](/docs/mcp-tools/)
- [PGSandbox architecture](/docs/architecture/)
- [Database migration testing before agent PRs](/blog/database-migration-testing-agent-pr/)
- [Owner and label policy for shared PGSandbox profiles](/blog/owner-label-policy-shared-pgsandbox-profiles/)
- [How to use cleanup_expired for stale resources](/blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/)

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "How to choose TTL values for Postgres sandboxes",
      "description": "Choose a sandbox TTL from expected task runtime, a review buffer when needed, and one bounded recovery attempt.",
      "step": [
        {"@type": "HowToStep", "name": "Measure task runtime", "text": "Measure the database work from sandbox creation through proof capture."},
        {"@type": "HowToStep", "name": "Add a review buffer", "text": "Add review time only when a reviewer needs the live sandbox rather than saved proof."},
        {"@type": "HowToStep", "name": "Add recovery margin", "text": "Reserve enough time for one normal retry after a predictable failure."},
        {"@type": "HowToStep", "name": "Apply profile limits", "text": "Use a positive ttlMinutes value that does not exceed the profile maximum."},
        {"@type": "HowToStep", "name": "Verify cleanup", "text": "Delete on success and preview expired cleanup with dryRun after interruptions."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {
          "@type": "Question",
          "name": "What is the default PGSandbox TTL?",
          "acceptedAnswer": {"@type": "Answer", "text": "PGSandbox uses a built-in default of 240 minutes when a profile does not set defaultTtlMinutes. Profiles can override it, and tasks may pass a positive ttlMinutes value up to maxTtlMinutes."}
        },
        {
          "@type": "Question",
          "name": "Does a Postgres sandbox disappear exactly when its TTL expires?",
          "acceptedAnswer": {"@type": "Answer", "text": "No. Expiry makes a tracked PGSandbox resource eligible for cleanup_expired. The database and role are removed when explicit deletion or cleanup succeeds."}
        },
        {
          "@type": "Question",
          "name": "Should long-running tests get a longer TTL?",
          "acceptedAnswer": {"@type": "Answer", "text": "Yes, when measured runtime requires it. Include setup, proof capture, and one recovery attempt, and keep the longer TTL specific to that workflow."}
        },
        {
          "@type": "Question",
          "name": "How long should a sandbox stay for code review?",
          "acceptedAnswer": {"@type": "Answer", "text": "Keep it only when review needs the live database. Start with 120 to 240 minutes for a same-day review, label the retention reason, and delete it when review no longer needs it."}
        }
      ]
    }
  ]
}
</script>
