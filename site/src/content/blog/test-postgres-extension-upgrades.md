---
title: "How to Test PostgreSQL Extension Upgrades Safely"
excerpt: "Test PostgreSQL extension update paths, target schema state, application behavior, migration compatibility, recovery, and cleanup in disposable databases."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-20"
updatedAt: "2026-07-20T06:00:00Z"
tags: ["Postgres", "PostgreSQL extensions", "database migrations", "database testing", "coding agents"]
category: "Engineering"
metaTitle: "Test PostgreSQL Extension Upgrades Safely"
metaDescription: "Test a PostgreSQL extension upgrade with path checks, twin sandboxes, application migrations, schema comparison, recovery, and cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-extension-upgrades/"
heroImageUrl: ""
featured: false
sortOrder: 142
---
A safe PostgreSQL extension upgrade test does more than run `ALTER EXTENSION UPDATE`. Inspect the installed and available versions, prove the actual update path, apply the update in a disposable database, run the application migration and behavior checks, compare the result with a new-database target lane, verify recovery, and remove the test resources.

The extra comparison matters. An extension update can finish successfully while leaving a different object shape or application behavior than a target version requested in a new database. For agent-written migrations, that difference belongs in the pull request evidence, not in a production incident.

PGSandbox does not add an extension-specific upgrade command. Its [MCP tool contract](/docs/mcp-tools/) provides the narrower building blocks needed to test the real PostgreSQL operation: task-scoped databases, bounded SQL, schema digests and diffs, repo migration commands, TTL metadata, and tracked cleanup.

## In this guide

- [Why the update command is not enough](#why-alter-extension-update-is-not-enough)
- [The two-lane proof contract](#use-a-two-lane-postgresql-extension-upgrade-test)
- [Build and inspect the upgrade lane](#1-match-the-target-postgresql-profile)
- [Run the extension and application migrations](#4-capture-a-baseline-and-run-the-postgresql-extension-upgrade)
- [Compare the target lane](#6-build-a-new-database-target-lane-and-compare)
- [Test recovery and cleanup](#7-test-recovery-and-cleanup)
- [Review the proof packet](#pr-ready-extension-upgrade-proof-packet)
- [Frequently asked questions](#frequently-asked-questions)

## PostgreSQL extension upgrade workflow at a glance

Use this seven-step workflow:

1. Select the same PostgreSQL major and extension package set used by the target environment.
2. Record the installed version, server-visible versions, and reachable update paths.
3. Create an upgrade-lane sandbox at the old version and capture a baseline.
4. Run `ALTER EXTENSION ... UPDATE TO ...` through the intended application authority boundary.
5. Apply the application migration and test the exact extension behavior it needs.
6. Build a new-database target lane and compare schema and behavior.
7. Exercise the recovery plan, record a compact proof packet, and delete both sandboxes.

This article calls that sequence the **Extension Upgrade Proof Contract**. Its purpose is to answer six separate questions: Is the target available? Is it reachable? Did the transition work? Does the upgraded state agree with a new target lane? Does the application still work? Can the team recover?

## Why `ALTER EXTENSION UPDATE` is not enough

PostgreSQL's [`ALTER EXTENSION` reference](https://www.postgresql.org/docs/current/sql-alterextension.html) defines the core command:

```sql
ALTER EXTENSION extension_name UPDATE [TO new_version];
```

The extension must provide an update script or a chain of scripts from the installed version to the requested version. If `TO` is omitted, PostgreSQL targets the `default_version` declared in the extension control file. The caller must own the extension, and the control file plus the SQL inside the scripts determine whether superuser authority is required.

That command proves one transition inside one database. It does not prove all of the following:

- the target package files exist on every environment that matters;
- PostgreSQL will choose the path you expected;
- the update works from every deployed starting version;
- the application migration is compatible with both old and new states;
- a new-database target lane produces the same expected review-relevant schema;
- the extension author supplied a downgrade path;
- a dump or backup can actually be restored with compatible extension files.

The earlier guide to [testing PostgreSQL extensions locally](/blog/test-postgres-extensions-locally/) covers availability, installation, behavior, migration compatibility, and cleanup. Upgrade testing adds a state transition between two versions. That transition needs its own evidence.

### Should the extension update always run before the app migration?

No. Update the extension first when the application migration depends on functions, types, operators, or behavior introduced by the target extension version. If the application migration prepares data or objects required by the extension update, the order may differ. Write the dependency down and test the exact production sequence.

A PostgreSQL major-version upgrade is a separate layer. PostgreSQL's [`pg_upgrade` documentation](https://www.postgresql.org/docs/current/pgupgrade.html) says matching extension shared objects must be installed for the new server and notes that extension SQL updates may still be needed after the engine upgrade. More generally, compatible supporting files must exist, including matching shared libraries for extensions that contain compiled code. Do not collapse server package installation, an engine upgrade, an extension SQL update, and an application migration into one reversible step.

Preloaded observability modules add another boundary. The guide to [testing `pg_stat_statements` in disposable agent sandboxes](/blog/test-pg-stat-statements-agent-sandboxes/) shows how to verify server preload state, database-local registration, task-role query visibility, and clone exclusion separately.

## Use a two-lane PostgreSQL extension upgrade test

Treat the upgrade as six gates instead of one command:

| Gate | Question | Evidence |
| --- | --- | --- |
| Inventory | What is installed, and what versions can this server see? | `pg_extension` and `pg_available_extension_versions` |
| Path | Can the installed version reach the target, and which route will PostgreSQL choose? | `pg_extension_update_paths()` |
| Transition | Does the exact update complete under the intended role? | `ALTER EXTENSION ... UPDATE TO ...` result and post-update version |
| Convergence | Does the upgraded database agree with a new-database target lane? | Twin-sandbox schema digests and targeted catalog checks |
| Application | Do the real migration and extension-dependent behavior still work? | Repo command, bounded SQL assertions, and schema diff |
| Recovery | Can the team reverse or restore the change using a tested method? | Verified downgrade path or restore rehearsal, followed by cleanup |

The convergence gate catches the failure most upgrade guides miss: a transition that exits successfully but does not reach the intended target state. A twin-sandbox test compares two routes to the same intended state:

```text
Upgrade lane: old extension -> update scripts -> target version -> app migration
Target lane:  new database -> request target version    -> app migration
```

The open-source [`pg_validate_extupgrade` project](https://github.com/rjuju/pg_validate_extupgrade) uses the same underlying testing idea for extension authors: explicit source and target versions, pre-update fixture queries, and comparisons between post-install and post-upgrade results. The workflow here adapts that principle for application teams and agent migration reviews.

If the lanes do not produce equivalent schema and application results, the upgrade needs investigation even when both commands exit successfully.

## 1. Match the target PostgreSQL profile

Extension versions come from the files installed alongside a particular PostgreSQL server. Start by selecting the same PostgreSQL major, package source, and profile configuration used by the target environment.

With PGSandbox, call `list_profiles`, then use an explicit `postgresVersion` or named `profile`. Next, call `list_extensions` at profile scope:

```json
{
  "postgresVersion": "18"
}
```

Profile-scoped discovery reports the extension's default version through `pg_available_extensions`. It is a useful preflight, but it does not show every update edge. PostgreSQL's [`pg_available_extension_versions` view](https://www.postgresql.org/docs/current/view-pg-available-extension-versions.html) is the more detailed inventory for upgrade work.

Do not copy a package-version assumption from a laptop into the proof packet. Record the resolved PGSandbox profile, resolved PostgreSQL major, extension name, and exact source and target extension versions.

## 2. Create the upgrade lane at the old version

Prefer creating the upgrade lane from a representative sanitized clone or restore of the deployed old-version state. That captures historical update effects, approved extension configuration, and the application objects that a synthetic installation may miss.

If a representative baseline is unavailable, create a disposable database without asking PGSandbox to install the extension at its default version:

```json
{
  "postgresVersion": "18",
  "nameHint": "extension-upgrade-lane",
  "ttlMinutes": 60,
  "owner": "agent-extension-upgrade",
  "labels": {
    "repo": "example-app",
    "workflow": "extension-upgrade"
  }
}
```

Then use `run_sql` to request the package-defined source version explicitly:

```sql
CREATE EXTENSION extension_name VERSION 'old_version';
```

This only works when the selected server has a suitable base installation script or an installable chain to that version. PostgreSQL's [extension packaging documentation](https://www.postgresql.org/docs/current/extend-extensions.html) explains that extension script filenames encode versions and transitions. PostgreSQL may satisfy the request through an installation script plus update or downgrade scripts. Inspect the installed package files and update graph before treating the result as a standalone source-version installation, and do not claim a synthetic lane reproduces production drift or user-modified configuration.

Run the installation through the same authority boundary intended for the application workflow. PGSandbox's sandbox-scoped `run_sql` uses the generated database role. When an extension has the default `superuser = true`, `trusted = true` allows a non-superuser with `CREATE` privilege on the database to install or update it, with the script executed as the bootstrap superuser. When `superuser = false`, the caller instead needs the privileges required by the script commands. A privilege failure is not a reason to hand the coding agent an admin URL. It is evidence that extension lifecycle belongs in a separate platform step.

After installation, load the pre-change application schema and the smallest data fixture needed for behavior checks. Before the update, confirm `extversion`, run the old application's key assertion, and record representative legacy or configuration rows with bounded queries. Rerun the corresponding checks after the transition so a failure can be attributed to the update instead of a broken fixture. The broader [database migration testing workflow](/blog/database-migration-testing-agent-pr/) shows how to keep this state task-scoped and reviewable.

Create one upgrade lane for every source version still present in supported environments. Do not infer that a successful `1.4 -> 2.0` transition proves `1.2 -> 2.0`; PostgreSQL can select a different chain for each starting version.

## 3. Inspect versions and the actual update path

Run these read-only queries in the upgrade lane:

```sql
SELECT extname, extversion
FROM pg_extension
WHERE extname = 'extension_name';

SELECT version, installed
FROM pg_available_extension_versions
WHERE name = 'extension_name'
ORDER BY version;

SELECT source, target, path
FROM pg_extension_update_paths('extension_name')
ORDER BY source, target;
```

Use bounded SQL output for the evidence packet. The [bounded `run_sql` guide](/blog/postgres-run-sql-bounded-results/) explains how to preserve ordered results without dumping an unbounded catalog response into an agent transcript.

These queries answer different questions:

- [`pg_extension.extversion`](https://www.postgresql.org/docs/current/catalog-pg-extension.html) is the version currently installed in this database.
- [`pg_available_extension_versions`](https://www.postgresql.org/docs/current/view-pg-available-extension-versions.html) lists versions whose support files are visible to the selected server.
- [`pg_extension_update_paths()`](https://www.postgresql.org/docs/current/extend-extensions.html) shows whether one version can reach another and the route PostgreSQL would use.

A visible target version is not proof of reachability. The path can be `NULL` for a source-target pair. PostgreSQL also does not interpret version labels as semantic versions. It chooses the available route that requires the fewest update scripts.

That last rule deserves inspection. The PostgreSQL documentation warns that a supplied downgrade script can become part of a shorter route to a later version. If a downgrade removes irreplaceable objects, a surprising shortest path can be destructive. Record the exact `path` value before executing the change.

## 4. Capture a baseline and run the PostgreSQL extension upgrade

Capture the pre-update state with `schema_digest` or a named schema snapshot. PGSandbox schema digests include installed extension names and versions and are independent of generated sandbox database names, so they can be compared across task databases.

Then run the exact target command:

```sql
ALTER EXTENSION extension_name UPDATE TO 'target_version';
```

Always name the target version in an automated proof. Omitting `TO` delegates the choice to the current control file's default, which can change when packages change.

Immediately verify the result:

```sql
SELECT extname, extversion
FROM pg_extension
WHERE extname = 'extension_name';
```

PostgreSQL [executes extension installation and update scripts within a transaction block](https://www.postgresql.org/docs/current/extend-extensions.html), so an `ALTER EXTENSION` statement that fails does not commit partial transactional database changes. The scripts cannot issue transaction-control commands or commands prohibited in a transaction block, such as `VACUUM`. A successful commit still needs application and recovery testing.

The mutating PGSandbox payload makes the authority boundary explicit:

```json
{
  "databaseId": "upgrade-lane-id",
  "readonly": false,
  "rowLimit": 20,
  "sql": "ALTER EXTENSION extension_name UPDATE TO 'target_version'"
}
```

Use `diff_schema_snapshot` or `schema_diff` to record extension, relation, index, constraint, and view changes. The [schema snapshot guide](/blog/postgres-schema-snapshots-agent-migration-reviews/) explains why a digest alone is insufficient: a checksum proves difference, while the object diff explains it.

## 5. Run the application migration and behavior checks

An extension upgrade can preserve its own catalog entry and still break the application contract. Run the real migration command against the upgrade lane, then test the exact feature the application uses.

Examples include:

- calling a function with the argument types used by production code;
- creating the same operator-class index declared by the migration;
- inserting and reading the extension-backed data type;
- checking a renamed function, changed return type, or altered default;
- running the query plan or constraint behavior the application depends on.

Keep each assertion deterministic. “The extension loaded” is weaker than “this exact function call returned the expected value” or “this migration created the expected index and the query used it.”

Record the repo command, exit code, bounded output, post-migration schema diff, and the extension-specific assertion. Do not include a credential-bearing database URL. PGSandbox returns redacted variants by default; repo-command tools inject sandbox credentials into the child process, while raw connection strings require an explicit credential-bearing response.

## 6. Build a new-database target lane and compare

Create a second sandbox on the same profile. Install the target version directly:

```sql
CREATE EXTENSION extension_name VERSION 'target_version';
```

`CREATE EXTENSION ... VERSION` requests the target version, but PostgreSQL may satisfy that request by installing another base version and following update scripts. Call this a standalone fresh-install comparison only when package inspection confirms a standalone `extension--target_version.sql` script. Otherwise, it is a new-database target lane: still useful for comparison, but not independent of the update chain.

Apply the same application baseline and migration used in the upgrade lane. Run the same behavior assertions. Then capture `schema_digest` from both sandboxes.

Compare the digests and explain every difference inside the review-relevant schema contract. Require exact checksum equality only when the fixture and expected target state make that appropriate. An explained difference may come from a version-specific schema choice or environment-owned object. An unexplained difference can reveal a missed update script, stale object, divergent privilege, or migration assumption.

PGSandbox's digest covers extension versions and review-relevant relations, columns, constraints, indexes, and views. It does not include extension configuration-table rows or prove every extension function, operator, type behavior, or data semantic. Compare configuration data with identical bounded queries and run the same application-level assertions in both lanes even when their checksums match.

This is also where restore behavior belongs. PostgreSQL's [extension documentation](https://www.postgresql.org/docs/current/extend-extensions.html) says `pg_dump` normally records `CREATE EXTENSION` instead of dumping each extension member object. The destination must have supporting files capable of creating the dumped version, including the required control and SQL files, shared libraries where applicable, and prerequisite extensions. A logical dump does not embed those files and is not a self-contained extension rollback artifact.

## 7. Test recovery and cleanup

PostgreSQL has no universal “undo the last extension upgrade” command. A reverse transition exists only if the extension author supplies an appropriate downgrade script. Check [`pg_extension_update_paths()`](https://www.postgresql.org/docs/current/extend-extensions.html) for that route, but do not assume its presence makes rollback safe. Test it against disposable state and verify the application afterward.

When no trustworthy downgrade path exists, recovery usually means restoring a tested backup or rebuilding from a known source. Use a separate recovery lane: restore the verified pre-upgrade backup into a fresh compatible sandbox, ensure the required old extension files exist, and rerun the old application assertion. A PGSandbox schema snapshot is review metadata, not a restorable database backup. Document the recovery boundary before production rollout, not after an upgrade fails.

Delete both PGSandbox databases when the proof is complete. The known `databaseId` is the safest selector:

```json
{
  "databaseId": "sandbox-id"
}
```

TTL is a fallback for interrupted work, not the successful completion path. Explicit deletion removes the tracked database and generated role together.

## PR-ready extension upgrade proof packet

Keep the final artifact compact:

```text
PostgreSQL extension upgrade proof:
- profile: local-pg18
- extension: extension_name
- sourceVersionsTested: old_version, other_deployed_version
- targetVersion: target_version
- selectedPath: old_version--target_version
- upgradeRoleBoundary: sandbox role | separate platform step
- upgradeResult: success
- applicationMigrationExitCode: 0
- behaviorChecks: <named deterministic assertions>
- preUpdateBehavior: pass
- postUpdateBehavior: pass
- targetLaneBehavior: pass
- convergence: exact match | explained schema/data differences
- recoveryLane: tested downgrade path | tested restore procedure
- cleanup: both databases and generated roles deleted
```

Use one packet per deployed source version or include a small source-version matrix. The packet makes the decision legible to a reviewer: which transitions were tested, which authority performed them, what the application proved, how the new-database target lane compared, and how the team would recover.

## Common PostgreSQL extension upgrade mistakes

### Updating to an implicit default

`ALTER EXTENSION name UPDATE` follows the control file's current default. Pin `TO 'target_version'` in a reproducible test and rollout plan.

### Treating available as reachable

`pg_available_extension_versions` proves that support files are visible. It does not prove a path from the installed version. Inspect `pg_extension_update_paths()`.

### Assuming version labels are semantic versions

PostgreSQL treats extension versions as labels and chooses the route with the fewest scripts. Review the path instead of sorting labels in application code.

### Testing only a new target database

A new target database does not reproduce the deployed starting state. Test both the deployed transition and the target lane, and remember that `CREATE EXTENSION ... VERSION` may itself follow an update chain.

### Treating command success as application proof

The extension catalog can show the target version while an application function, type, index, or migration has changed behavior. Run the real application checks.

### Promising rollback without testing it

Downgrade scripts are optional. A dump also depends on the [required extension support files](https://www.postgresql.org/docs/current/extend-extensions.html) at restore time. Name and rehearse the actual recovery path.

## Frequently asked questions

### How do I upgrade a PostgreSQL extension?

Use `ALTER EXTENSION extension_name UPDATE TO 'target_version'` as the extension owner. The extension must supply a suitable update script or chain, and its control settings and SQL may require elevated authority.

### How do I see PostgreSQL extension update paths?

Query `pg_extension_update_paths('extension_name')`. It returns source, target, and selected path values for known versions; a `NULL` path means the pair is not reachable with the installed scripts.

### Why does PostgreSQL say there is no update path?

The target version may be visible while the extension package lacks a script chain from the installed source version. Inspect `pg_extension_update_paths()`, install the correct extension package and scripts, and retry from a disposable baseline. Do not edit `pg_extension.extversion` to fabricate a path.

### Can I roll back a PostgreSQL extension upgrade?

Only when the extension supplies a safe reverse update path or you have another tested recovery method. PostgreSQL does not provide a universal automatic rollback command for a successfully committed extension update.

### Should an application migration update the extension?

Only when the migration role is intentionally authorized to own and update that extension in every target environment. Otherwise, make the extension update a separate platform step and test the application migration against the prepared target state.

### Does a PostgreSQL major-version upgrade also upgrade extensions?

Not as one automatic operation. Install extension support files that match the new PostgreSQL server, complete the engine-upgrade procedure, then run any required SQL-level extension updates according to the extension and provider instructions.

### Why compare an upgraded database with a new target database?

The comparison detects state that only one route creates or leaves behind. Explained schema differences plus matching application behavior give reviewers stronger evidence about the deployed transition. It is a standalone-install comparison only when the package provides a standalone target-version installation script.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "How to Test PostgreSQL Extension Upgrades Safely",
      "description": "Test a PostgreSQL extension upgrade with path inspection, disposable twin sandboxes, application migration checks, schema convergence, recovery, and cleanup.",
      "datePublished": "2026-07-20",
      "dateModified": "2026-07-20",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Match the target profile", "text": "Select the target PostgreSQL major and extension package set."},
        {"@type": "HowToStep", "position": 2, "name": "Create the upgrade lane", "text": "Create a disposable database and install the deployed source extension version."},
        {"@type": "HowToStep", "position": 3, "name": "Inspect the update path", "text": "Record installed, available, and reachable extension versions before executing the change."},
        {"@type": "HowToStep", "position": 4, "name": "Run the extension update", "text": "Capture a baseline, update to an explicit target version, and verify the resulting catalog and schema state."},
        {"@type": "HowToStep", "position": 5, "name": "Test the application", "text": "Run the real application migration and deterministic extension-dependent behavior checks."},
        {"@type": "HowToStep", "position": 6, "name": "Compare with a new target database", "text": "Create a new-database target lane and compare schema digests, bounded data, and application behavior."},
        {"@type": "HowToStep", "position": 7, "name": "Test recovery and clean up", "text": "Exercise the documented recovery method, record proof, and delete both sandboxes."}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test PostgreSQL Extension Upgrades Safely", "item": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-extension-upgrades/"}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do I upgrade a PostgreSQL extension?", "acceptedAnswer": {"@type": "Answer", "text": "Run ALTER EXTENSION extension_name UPDATE TO target_version as the extension owner after verifying that the required update path and authority exist."}},
        {"@type": "Question", "name": "How do I see PostgreSQL extension update paths?", "acceptedAnswer": {"@type": "Answer", "text": "Query pg_extension_update_paths with the extension name. It reports source, target, and chosen path values; NULL means no available path."}},
        {"@type": "Question", "name": "Why does PostgreSQL say there is no update path?", "acceptedAnswer": {"@type": "Answer", "text": "The target version may be visible while the package lacks a script chain from the installed source version. Install the correct package and scripts; do not edit the extension catalog to fabricate a path."}},
        {"@type": "Question", "name": "Can I roll back a PostgreSQL extension upgrade?", "acceptedAnswer": {"@type": "Answer", "text": "Only with a safe extension-supplied reverse path or another tested recovery method. PostgreSQL has no universal automatic rollback for a committed extension update."}},
        {"@type": "Question", "name": "Should an application migration update the extension?", "acceptedAnswer": {"@type": "Answer", "text": "Only when the migration role is intentionally authorized to own and update the extension everywhere. Otherwise, use a separate platform step and test that boundary."}},
        {"@type": "Question", "name": "Does a PostgreSQL major-version upgrade also upgrade extensions?", "acceptedAnswer": {"@type": "Answer", "text": "Not as one automatic operation. Install matching extension support files for the new server, complete the engine upgrade, then run any required SQL-level extension updates."}},
        {"@type": "Question", "name": "Why compare an upgraded database with a new target database?", "acceptedAnswer": {"@type": "Answer", "text": "The comparison reveals schema, data, or behavior created by only one route. It is a standalone-install comparison only when the package provides a standalone target-version installation script."}}
      ]
    }
  ]
}
</script>
