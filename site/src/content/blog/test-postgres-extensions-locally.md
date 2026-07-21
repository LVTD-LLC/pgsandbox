---
title: "How to Test PostgreSQL Extensions in Disposable Sandboxes"
excerpt: "Test PostgreSQL extension availability, installation, behavior, migration compatibility, and cleanup in a disposable task database."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-19"
updatedAt: "2026-07-19T06:00:00Z"
tags: ["Postgres", "PostgreSQL extensions", "database testing", "MCP", "coding agents"]
category: "Engineering"
metaTitle: "Test PostgreSQL Extensions Locally in Sandboxes"
metaDescription: "Test PostgreSQL extensions locally with profile discovery, disposable installation, behavior checks, migration proof, and cleanup."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-extensions-locally/"
heroImageUrl: ""
featured: false
sortOrder: 141
---
To test PostgreSQL extensions locally, first confirm that the selected PostgreSQL profile exposes the extension in `pg_available_extensions`. Create a disposable database with the extension requested, verify its installed version in `pg_extension`, run an application-level behavior check, test the real migration or restore path, capture schema evidence, and delete the sandbox.

Do not stop at `CREATE EXTENSION` succeeding. That proves installation in one database. It does not prove that the extension exists on every target PostgreSQL major, that the application uses it correctly, that a restore can reproduce it, or that the task left no shared state behind.

PGSandbox turns those concerns into a reviewable local workflow. The agent works through a generated role in a tracked database, while lifecycle operations remain behind the MCP server's admin connection.

## PostgreSQL extension test workflow at a glance

Use this sequence for an application or migration that depends on an extension:

1. Select the same PostgreSQL major or explicit profile used by the target environment.
2. Call `list_extensions` before creation and confirm that the extension is available.
3. Call `create_database` with the extension in `extensions`.
4. Call sandbox-scoped `list_extensions` and record the installed version.
5. Run one behavior test that exercises the feature the application needs.
6. Run the actual migration, seed, or clone path and capture the schema result.
7. Delete the sandbox, or retain it only under an intentional TTL and owner policy.

This is the short version. The rest of the guide explains why each step exists and what evidence belongs in a pull request.

## Use the five-gate Extension Proof Contract

Extension testing becomes clearer when you separate five gates that are often collapsed into one:

| Gate | Question | Evidence |
| --- | --- | --- |
| Availability | Does this PostgreSQL profile have the extension's control and script files? | Profile-scoped `list_extensions` result |
| Installation | Can the sandbox role install the requested extension in a new database? | `create_database.installedExtensions` and sandbox-scoped `list_extensions` |
| Behavior | Does the exact function, type, operator, or index needed by the application work? | Small deterministic `run_sql` assertion |
| Migration compatibility | Can the real migration, schema clone, or restore reproduce the dependency? | Repo command result plus schema digest or diff |
| Cleanup | Did the test remove its database and generated role without touching unrelated resources? | `delete_database` result or intentional TTL record |

This framework is the information-gain layer missing from most extension installation guides. An extension can pass availability and fail installation because it needs more privilege. It can pass installation and fail behavior because the application assumes a different version. It can pass a hand-written smoke test and fail a clone because the source archive carries a server-specific extension entry.

Treat the gates independently. The final result is easier for an agent to diagnose and easier for a reviewer to trust.

## 1. Match the PostgreSQL profile before testing extensions

Extension availability belongs to a PostgreSQL installation and profile, not to the abstract name "Postgres." Package sets differ across operating systems, distribution repositories, container images, managed providers, and major versions.

Start by selecting the same major or explicit profile that matters to the task. The [local PostgreSQL version guide](/blog/how-to-use-local-postgres-versions-with-coding-agents/) explains when to use `postgresVersion` and when to pin a named profile.

For a managed local PostgreSQL 18 check, discover the profile first:

```json
{
  "includeDiscoveredLocal": true
}
```

Then ask PGSandbox for the extensions visible on that major:

```json
{
  "postgresVersion": "18"
}
```

That second payload is for `list_extensions`. A profile-scoped result includes `resolvedProfile`, `resolvedPostgresVersion`, and `availableExtensions`. Each available entry comes from PostgreSQL's `pg_available_extensions` view and includes the extension name, default version, optional installed version, and control-file comment.

PostgreSQL's [official catalog view documentation](https://www.postgresql.org/docs/current/view-pg-available-extensions.html) draws the important boundary: `pg_available_extensions` lists extensions available for installation, while `pg_extension` shows extensions installed in the current database.

Do not treat this discovery result as proof that the application dependency is active. It clears only the availability gate.

### Available is not the same as installed

PostgreSQL extension packages normally place a control file and SQL scripts where the server can find them. That makes the extension available. `CREATE EXTENSION` then runs the extension script and registers the resulting objects in one database.

The distinction matters because extensions are database-local registrations. PostgreSQL's [extension packaging documentation](https://www.postgresql.org/docs/current/extend-extensions.html) states that extensions are known only within one database; cluster-wide objects such as roles and tablespaces cannot be extension members.

The server package can therefore be present while a fresh task database has no installed extension entry. That is normal and is exactly what a clean sandbox should reveal.

## 2. Create a disposable database with the extension requested

After availability passes, request the extension as part of sandbox creation:

```json
{
  "postgresVersion": "18",
  "nameHint": "pg-trgm-extension-proof",
  "ttlMinutes": 45,
  "owner": "agent-extension-check",
  "labels": {
    "repo": "example-app",
    "workflow": "extension-proof"
  },
  "extensions": ["pg_trgm"]
}
```

PGSandbox trims extension names, lowercases them, removes duplicates, and accepts only single identifiers containing letters, numbers, underscores, or hyphens. It checks the selected target's `pg_available_extensions` view before executing `CREATE EXTENSION IF NOT EXISTS`.

The installation runs through the generated sandbox role connection, not the admin connection. That detail gives the test practical value: it verifies the authority available inside the task database instead of silently using lifecycle credentials to make the check pass.

The [MCP tool contract](/docs/mcp-tools/) documents two useful failure branches:

- `invalid_extensions` means the name is malformed, unavailable on the selected profile, or failed normal installation.
- `extension_setup_required` means PostgreSQL reported a recognized server-level requirement, such as preload configuration.

PGSandbox removes the new database and generated role if requested extension installation fails during `create_database`. The implementation creates the role and database, attempts installation, and then rolls both resources back on error. That rollback behavior is worth testing because a failed extension check should not accumulate half-created task databases.

### Privilege failures are useful evidence

PostgreSQL's [`CREATE EXTENSION` documentation](https://www.postgresql.org/docs/current/sql-createextension.html) says a trusted extension can be installed by a user that has `CREATE` privilege on the current database. Extensions that are not trusted generally require superuser privileges.

PGSandbox does not hide that boundary by installing every extension through its admin connection. If the task role cannot install the extension under the selected profile's rules, the workflow should fail and tell you that the environment needs deliberate server setup.

Do not repair the failure by granting the coding agent a superuser login. Configure an explicit profile for the extension, document the requirement, or choose a different test environment. The [PGSandbox architecture](/docs/architecture/) keeps lifecycle authority and task SQL authority separate for this reason.

## 3. Verify the installed extension and version

A successful creation response returns normalized names under `installedExtensions`. Follow it with sandbox-scoped discovery:

```json
{
  "databaseId": "sandbox-id"
}
```

This `list_extensions` call reports both available and installed entries for the selected sandbox. Record the installed name and version, not only a boolean.

PostgreSQL's [`pg_extension` catalog](https://www.postgresql.org/docs/current/catalog-pg-extension.html) stores the extension name, owner, namespace, relocatability flag, and version. PGSandbox reads `extname` and `extversion` from that catalog for its installed extension result.

If you need a small SQL assertion as well, use a bounded read:

```json
{
  "databaseId": "sandbox-id",
  "readonly": true,
  "rowLimit": 20,
  "sql": "SELECT extname, extversion FROM pg_extension WHERE extname = 'pg_trgm'"
}
```

Do not use `CREATE EXTENSION IF NOT EXISTS` as your verification query. PostgreSQL warns that `IF NOT EXISTS` only suppresses the error when an extension with that name already exists; it does not guarantee that the existing extension matches what would have been created.

Version evidence matters when a migration uses a function or operator introduced in a newer extension release. "Installed" is too coarse for a compatibility claim.

## 4. Test the behavior the application actually needs

An installation check should be followed by one narrow behavior test. Choose the function, type, operator, index method, or migration statement that makes the extension necessary for this application.

For `pg_trgm`, PostgreSQL's [official module documentation](https://www.postgresql.org/docs/current/pgtrgm.html) exposes `similarity(text, text)` and supports GiST and GIN indexes through trigram operator classes. A useful smoke test can prove both function execution and the index definition the application migration expects.

Create a small table:

```json
{
  "databaseId": "sandbox-id",
  "sql": "CREATE TABLE extension_probe (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, body text NOT NULL)",
  "rowLimit": 10
}
```

Create the intended index:

```json
{
  "databaseId": "sandbox-id",
  "sql": "CREATE INDEX extension_probe_body_trgm_idx ON extension_probe USING GIN (body gin_trgm_ops)",
  "rowLimit": 10
}
```

Then run a read-only function assertion:

```json
{
  "databaseId": "sandbox-id",
  "readonly": true,
  "rowLimit": 5,
  "sql": "SELECT similarity('postgres sandbox', 'postgres sandboxes') AS score"
}
```

The score itself is not a benchmark. It proves that the expected function resolves and executes under the sandbox role. The index creation proves that the intended operator class is available to the migration.

For another extension, replace these statements with the smallest behavior that expresses the real dependency. A `vector` application should test the exact column type and operator used by its migration. An `hstore` application should test its key/value operators. A foreign-data wrapper needs a more deliberate server and credential boundary than a pure SQL function.

Avoid generic checks such as `SELECT 1`. They prove the database connection, not the extension.

## 5. Run the real migration or clone path

Hand-written SQL can clear the behavior gate while the repository migration still fails. Run the real command against the same sandbox after the focused smoke test.

The [database migration testing workflow](/blog/database-migration-testing-agent-pr/) covers the full repo-command path. For extension-dependent work, add three fields to the proof:

- selected PostgreSQL profile and major,
- extension name and installed version,
- whether the migration creates the extension or expects it to exist already.

Use `schema_digest`, `schema_diff`, or a named schema snapshot around the command. PGSandbox digests include extension names and versions, and schema diffs report added, removed, or changed extensions. The [schema snapshot guide](/blog/postgres-schema-snapshots-agent-migration-reviews/) shows how to turn the before/after shape into compact reviewer evidence.

This catches a common ownership ambiguity. If application migrations contain `CREATE EXTENSION`, the migration runner needs enough authority for that extension. If platform setup installs the extension beforehand, the migration should test against an environment that reflects that contract instead of gaining extra privilege for convenience.

### Cloning needs an explicit extension policy

`clone_database` accepts two different extension controls:

- `extensions` installs target extensions in the empty sandbox before `pg_restore`.
- `excludeSourceExtensions` removes selected source extension entries from the restore archive.

PGSandbox skips the source `pg_stat_statements` extension entry by default because a sandbox role commonly cannot create that observability extension. Add other environment-specific source extensions to `excludeSourceExtensions` only after deciding that the application schema does not depend on them.

If the task needs query collection rather than merely excluding source observability state, use the dedicated guide to [testing `pg_stat_statements` in an agent sandbox](/blog/test-pg-stat-statements-agent-sandboxes/). It separates server preload and restart state from database registration, task-role visibility, workload proof, and cleanup.

Example schema-only clone shape:

```json
{
  "sourceDatabaseUrl": "<provide through a secret input>",
  "postgresVersion": "18",
  "schemaOnly": true,
  "extensions": ["pg_trgm"],
  "excludeSourceExtensions": ["auto_explain"],
  "nameHint": "extension-clone-proof",
  "ttlMinutes": 45
}
```

Never paste a real source database URL into a prompt, issue, log, or tracked file. The placeholder above marks a secret input boundary, not a value to replace in documentation.

Installing target extensions before restore lets restored objects refer to their types, functions, and operator classes. Excluding a source extension is different: it tells the clone workflow that the target test does not need that source-only registration. Mixing those two intentions produces fragile restores.

## 6. Record a PR-ready extension proof packet

A reviewer should not need the sandbox password or raw command transcript. Record the stable evidence:

```text
PostgreSQL extension proof:
- profile: local-pg18
- resolvedPostgresVersion: 18
- extension: pg_trgm
- availableDefaultVersion: <from list_extensions>
- installedVersion: <from sandbox list_extensions>
- behavior: similarity() resolved; GIN gin_trgm_ops index created
- repoMigrationExitCode: 0
- schemaDiff: expected extension/index changes only
- cleanup: sandbox database and role deleted
```

Keep credential-bearing connection strings out of the packet. Use the sandbox id only when the review system is private and the id helps an operator inspect retained metadata.

If a gate fails, include the normalized error code and the next action. The [Postgres MCP error handling guide](/blog/postgres-mcp-server-error-handling-coding-agents/) explains why agents should branch on stable codes rather than retrying the same request with broader privileges.

## 7. Delete the extension sandbox

Delete the known sandbox when the proof is complete:

```json
{
  "databaseId": "sandbox-id"
}
```

This `delete_database` call removes the tracked database and generated role through the lifecycle boundary. Do not manually drop only the database and leave the role or PGSandbox metadata behind.

TTL is a recovery control, not the normal completion path. A 45-minute TTL can clean up after an interrupted agent session, but a successful workflow should still delete its known resource. Shared profiles should also attach a stable owner and low-cardinality labels so cleanup remains scoped.

The cleanup gate is part of extension correctness because extension tests often fail during environment setup. A workflow that proves the SQL but leaves a trail of failed databases is not production-ready automation.

## Common PostgreSQL extension testing mistakes

### Testing whatever version happens to be on the laptop

The test can pass locally and fail on the target major or image. Select a version or profile explicitly and record the resolved version.

### Treating availability as installation

An entry in `pg_available_extensions` means PostgreSQL can see the extension package. Check `pg_extension` in the sandbox to prove installation.

### Using admin credentials for behavior tests

That hides privilege and ownership failures. Install and run application checks through the task database role whenever the real workflow expects that role to own the objects.

### Checking only the extension name

Capture `extversion`, then exercise the exact feature used by the application. Extension names stay stable while behavior and available update paths can change.

When the dependency is already deployed, use the [PostgreSQL extension upgrade testing workflow](/blog/test-postgres-extension-upgrades/) to inspect the packaged path and compare an upgraded sandbox with a new-database target lane.

### Ignoring clone and restore ordering

An extension-dependent schema can fail during restore even after a manual smoke test passes. Install required target extensions before restore and exclude only source-specific entries that the target does not need.

### Keeping raw database URLs in test artifacts

Extension proof needs profile, version, catalog state, behavior, schema results, and cleanup. It does not need a password-bearing URL.

## Frequently asked questions

### How do I list installed PostgreSQL extensions?

Query `pg_extension` in the target database, or use sandbox-scoped `list_extensions` with a PGSandbox `databaseId` or `databaseName`. `pg_extension` shows installed entries and versions. `pg_available_extensions` answers a different question: which extension packages PostgreSQL can install.

### Are PostgreSQL extensions installed per database or per server?

The extension package files are available to a PostgreSQL installation, while `CREATE EXTENSION` registers the extension and its objects in one database. PostgreSQL documents extensions as database-local; roles, databases, and tablespaces cannot be extension members because those objects are cluster-wide.

### Why is an extension available but not installable by the sandbox role?

Availability proves that PostgreSQL can find the extension control and script files. Installation can still require superuser authority, a trusted-extension declaration, a server package, or preload configuration. Treat that as a profile setup decision instead of granting a coding agent broad cluster privileges.

### Should the application migration run CREATE EXTENSION?

Only if the migration role is intentionally allowed to install that extension in every target environment. Otherwise, make extension installation a documented platform prerequisite and test migrations against a sandbox where the extension is installed through that same setup boundary.

### How do I test an extension-dependent database clone?

Request required target extensions through `clone_database.extensions` so PGSandbox installs them before `pg_restore`. Use `excludeSourceExtensions` only for source-specific entries the sandbox does not need. Then verify the installed versions and run the application's behavior check after restore.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "How to Test PostgreSQL Extensions in Disposable Sandboxes",
      "description": "Test PostgreSQL extension availability, installation, application behavior, migration compatibility, and cleanup in a disposable task database.",
      "datePublished": "2026-07-19",
      "dateModified": "2026-07-19",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Match the PostgreSQL profile", "text": "Select the target PostgreSQL major or explicit profile and list the extensions available there."},
        {"@type": "HowToStep", "position": 2, "name": "Create an extension sandbox", "text": "Create a disposable database with the required extension requested through the task role boundary."},
        {"@type": "HowToStep", "position": 3, "name": "Verify the installed version", "text": "Use sandbox-scoped extension discovery or pg_extension to record the installed extension version."},
        {"@type": "HowToStep", "position": 4, "name": "Test application behavior", "text": "Run a small deterministic function, type, operator, or index check that represents the application's real dependency."},
        {"@type": "HowToStep", "position": 5, "name": "Test migration compatibility", "text": "Run the real migration or clone path and capture extension-aware schema evidence."},
        {"@type": "HowToStep", "position": 6, "name": "Record review evidence", "text": "Record the profile, PostgreSQL major, extension version, behavior result, schema result, and normalized failures without secrets."},
        {"@type": "HowToStep", "position": 7, "name": "Delete the sandbox", "text": "Delete the tracked database and generated role after the extension proof is complete."}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "How to Test PostgreSQL Extensions in Disposable Sandboxes", "item": "https://pgsandbox-mcp.lvtd.dev/blog/test-postgres-extensions-locally/"}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "How do I list installed PostgreSQL extensions?", "acceptedAnswer": {"@type": "Answer", "text": "Query pg_extension in the target database or use sandbox-scoped list_extensions. pg_extension shows installed entries and versions, while pg_available_extensions lists packages available for installation."}},
        {"@type": "Question", "name": "Are PostgreSQL extensions installed per database or per server?", "acceptedAnswer": {"@type": "Answer", "text": "Extension package files are available to a PostgreSQL installation, while CREATE EXTENSION registers the extension and its objects in one database."}},
        {"@type": "Question", "name": "Why is an extension available but not installable by the sandbox role?", "acceptedAnswer": {"@type": "Answer", "text": "Availability proves PostgreSQL can find the package files. Installation can still require superuser authority, a trusted-extension declaration, a server package, or preload configuration."}},
        {"@type": "Question", "name": "Should the application migration run CREATE EXTENSION?", "acceptedAnswer": {"@type": "Answer", "text": "Only when the migration role is intentionally allowed to install that extension in every target environment. Otherwise, treat installation as a platform prerequisite and test that boundary explicitly."}},
        {"@type": "Question", "name": "How do I test an extension-dependent database clone?", "acceptedAnswer": {"@type": "Answer", "text": "Request required target extensions so they are installed before pg_restore, exclude only source-specific entries the sandbox does not need, then verify installed versions and application behavior after restore."}}
      ]
    }
  ]
}
</script>
