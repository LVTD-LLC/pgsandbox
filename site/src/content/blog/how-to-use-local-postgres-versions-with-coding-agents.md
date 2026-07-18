---
title: "How to Use Local Postgres Versions with Coding Agents"
excerpt: "Use PGSandbox’s managed local versioned profiles to select specific PostgreSQL majors intentionally, avoid version-mismatch failures, and keep clone/migration validation predictable for coding agents."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-10"
updatedAt: "2026-07-10"
tags: ["Postgres", "MCP", "local Postgres", "versions", "AI agents"]
category: "Engineering"
metaTitle: "Use Local Postgres Versions with Coding Agents"
metaDescription: "A practical guide for selecting Postgres versions in PGSandbox, from profile selection and ensure_postgres to version mismatch recovery and safe rollback-proof agent SQL workflows."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/how-to-use-local-postgres-versions-with-coding-agents/"
heroImageUrl: ""
featured: false
sortOrder: 130
---
When coding agents need PostgreSQL features from a specific major version, choosing the wrong binary is a common source of hidden breakage. PGSandbox solves this with versioned profile selection instead of forcing agents to guess.

The core rule is simple: select by version where that matters, and let the profile contract carry the policy.

For local-first defaults, selecting `"postgresVersion": "18"` can resolve to a managed profile like `local-pg18`, while `"profile": "external-pg17"` keeps using a custom external instance.

## Why version selection is important for agent workflows

For agents, version matters in three places:

1. SQL syntax and planner behavior can vary by major version.
2. Extension packaging and availability can differ across majors.
3. Restore and dump compatibility depends on source and target server versions.

If an agent switches a test plan without pinning version, validation can look green on one run and fail on the next for reasons unrelated to product changes.

PGSandbox’s contract makes this explicit in the tool inputs and error codes, so version drift is recoverable instead of silent.

## What PGSandbox can run locally by default

With managed local runtime enabled, PGSandbox resolves versioned local profiles from major-specific definitions.

- versioned local profile pattern: `local-pg<major>`
- managed local data roots are split per major version, for example `~/.pgsandbox/postgres/versions/18/`

From the implementation and docs, local versions are discovered by numeric-major input and resolved from explicit bin-dir env vars first, then candidate locations, then `PATH`. The normalized major is what PGSandbox stores in selected profile names.

The practical outcome: if you request a version it can run, it can isolate that major; if binaries are missing, it returns a recoverable version diagnostic instead of proceeding on the wrong engine.

## Choose your selector strategy: profile, postgresVersion, or both

### Option A: select by `postgresVersion`

Use when:

- you want the latest available profile for a given major,
- you don’t need a fixed profile name,
- you are okay with managed local runtime discovery.

Example:

```json
{
  "nameHint": "agent-postgres-18-check",
  "postgresVersion": "18"
}
```

PGSandbox resolves to the matching managed profile and returns `resolvedProfile` and `resolvedPostgresVersion` in the response.

### Option B: select by `profile`

Use when:

- you use explicit external admin URLs,
- policy requires a custom host,
- or you need a known alias with extra constraints.

Example:

```json
{
  "profile": "external-pg17",
  "nameHint": "external-agent-pr-check"
}
```

### Option C: supply both

Only do this when they are intentionally coupled.

If both are supplied, they must match. If they do not, PGSandbox returns `version_mismatch`.

Use this only when you intentionally require `postgresVersion` as a contract check against a specific profile name.

## Safe workflow for local versioning in agents

1. Discover available versions and explicit profiles first.
2. Select the target selector style (`postgresVersion` for managed local majors, `profile` for explicit policies).
3. If missing binaries, run `ensure_postgres` (or set `PGSANDBOX_POSTGRES_<MAJOR>_BIN_DIR` / `PGSANDBOX_POSTGRES_BIN_DIR`).
4. Run lifecycle/read SQL on the selected selector.
5. Handle version failures with tool hints instead of blind retry.

Example command flow:

```bash
# Discover what versions and profiles are available.
pgsandbox list-profiles --include-discovered-local

# Prepare the requested local major if missing.
pgsandbox ensure-postgres --postgres-version 18

# Create a task-scoped sandbox on that major.
pgsandbox create-database --postgres-version 18 --name-hint "agent-check" --ttl-minutes 45

# Run a bounded agent SQL check in readonly mode.
pgsandbox run-sql --database-id "$DATABASE_ID" --readonly --row-limit 20 --sql "select 1"
```

The CLI and MCP tool flow is the same envelope shape. The difference is only the input shape and caller.

When the application command runs in a Docker container but the selected PostgreSQL profile runs on the host, the database URL needs a separate network decision. The [Docker container to host PostgreSQL guide](/blog/docker-connect-host-postgres/) explains `direct` versus `localContainer`, Docker Desktop and native Linux behavior, and the listener/HBA checks that still apply after the hostname resolves.

## Error codes that should shape your agent branching

Version and discovery errors are stable and should be treated as control-flow signals:

| Code | What it means | Recovery path |
| --- | --- | --- |
| `unknown_profile` | A named profile does not exist | call `list_profiles`, then retry with a valid profile
| `version_mismatch` | Both selector fields were supplied and disagree | omit one selector unless intentional exact pairing is required |
| `postgres_version_unavailable` | no configured profile advertises that major | choose a version from `list_profiles`, add explicit profile metadata, or rerun setup without `--admin-url` |
| `local_postgres_unavailable` | local binaries for requested major are missing | run `ensure_postgres`; set `PGSANDBOX_POSTGRES_<MAJOR>_BIN_DIR` or `PGSANDBOX_POSTGRES_BIN_DIR` |
| `restore_incompatible` | clone/restore target major cannot satisfy source constraints | target major must be same-or-newer than source; re-export compatible dump as needed |

This map is intentionally narrow. It keeps agent retries honest: if a failure is versioned, branch on the environment, not SQL.

## How to keep extension checks predictable

Extensions are often the place where version mismatches become expensive.

For profile-aware local tests, run extension discovery before creation:

```json
{
  "profile": "local-pg18"
}
```

Then request extension-specific operations only after you can see what the selected profile supports. This avoids failures where `CREATE EXTENSION` or restore metadata silently pushes a fallback path.

For local profile selection, remember that extension scripts can require server-level setup. PGSandbox returns `extension_setup_required` when the server package or preload path does not match the requested extension flow.

## A practical versioning decision matrix for agents

Before any major change, use this matrix:

- **Can existing workflow use managed local defaults?**
  - yes → prefer `postgresVersion`
  - no → use the exact `profile`
- **Need deterministic test reproducibility across runs and hosts?**
  - yes → pin an explicit `profile` when available
- **Hit `local_postgres_unavailable`?**
  - run `ensure_postgres` with `postgresVersion`, or fallback to explicit bin paths
- **Need local compatibility across developer machines?**
  - keep version choice in repo docs and include it in PR-ready SQL proof summaries

## Before you hand this to an agent, include these fields in the runbook

Add these to your PR notes so reviewers can trust the database proof:

- selector used (`profile` or `postgresVersion`)
- target Postgres major
- `pg_dump/pg_restore` path expectations when cloning
- extension inventory check result
- whether `ensure_postgres` was required
- normalized failure code and exact hint used when recovery happened

That record is what turns a temporary task check into a repeatable workflow.

## Common versioning mistakes

### Mistake 1: mixing `profile` and `postgresVersion` for convenience

That pattern hides intent. If you do it by accident, PGSandbox tells you with `version_mismatch`.

### Mistake 2: hardcoding default local profile assumptions

`local` and `local-pg18` are not interchangeable unless you intentionally request it. If feature parity differs across major versions, tests become noisy.

### Mistake 3: re-running a failing SQL mutation for a version issue

A version mismatch is not a query bug. Retry after correction of profile/version selection.

### Mistake 4: ignoring clone preflights

For agent cloning workflows, verify version compatibility up front and handle `restore_incompatible` before spending runtime on a long `pg_restore` retry path.

## Related pages you should read first

- [Postgres MCP Server Error Handling for Coding Agents](https://pgsandbox-mcp.lvtd.dev/blog/postgres-mcp-server-error-handling-coding-agents/) for the full error-code branching map.
- [Postgres MCP Tool Contract](https://pgsandbox-mcp.lvtd.dev/docs/mcp-tools/) for version and profile input rules.
- [How to Use Postgres EXPLAIN Plans for Agent SQL Review](https://pgsandbox-mcp.lvtd.dev/blog/postgres-explain-plan-agent-sql/) to separate planning failures from execution failures.

## FAQ

### Do I have to pass both `profile` and `postgresVersion`?

No. Most tasks pass one selector. Use both only when the profile and major intentionally must match.

### Can a single profile support many majors?

A single local profile maps to one major when managed locally and explicit values can still define broader behavior in project config. In practice, one major profile avoids ambiguity.

### What happens if `PGSANDBOX_POSTGRES_18_BIN_DIR` is set?

PGSandbox checks versioned env vars first for exact major requests. That gives deterministic local version control when package manager installs do not match your exact target.

### Does `ensure_postgres` install PostgreSQL?

For local majors it attempts package-manager installation where available, then starts the managed local runtime. If installation is unavailable, errors include actionable hint text and detected available methods.

## AEO-ready answer summary

If you want a one-line operational answer, use this in your agent note:

> Use `postgresVersion` for managed local majors and use explicit profiles for fixed external policies; if both selectors disagree, PGSandbox returns `version_mismatch` and you should rerun with one selector and the same recovery plan.
