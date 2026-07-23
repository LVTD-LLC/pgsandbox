---
title: "Run Integration Tests in a Disposable Postgres Database"
excerpt: "Run a repository test command against a fresh Postgres database, preserve useful failure evidence, and clean up the database with one bounded session."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-22"
updatedAt: "2026-07-22T06:00:00Z"
tags: ["Postgres", "integration testing", "test databases", "coding agents", "CI"]
category: "Engineering"
metaTitle: "Run Tests in a Disposable Postgres Database"
metaDescription: "Run integration tests against a fresh Postgres database with injected credentials, explicit cleanup, bounded output, and reproducible failure evidence."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/run-integration-tests-disposable-postgres-database/"
heroImageUrl: ""
featured: false
sortOrder: 144
---
To run integration tests in a disposable Postgres database, create the database before the test process starts, inject its connection settings into that process, run the repository's real test command, preserve the command's exit status and bounded output, then delete or deliberately retain the database. A TTL should remain as a recovery backstop if the session is interrupted.

PGSandbox packages that lifecycle into `pgsandbox with-database`. The command creates a fresh database and restricted login role, supplies standard connection variables to one child process, captures credential-redacted output, and applies an explicit cleanup policy. The application gets real PostgreSQL behavior without receiving the lifecycle credential that created the database.

This guide uses a **Session Proof Contract** with five fields: target, command, result, retention, and cleanup. If a CI job or coding agent cannot report those five fields, it has not produced a reproducible database-test result.

## In this guide

- [Run the basic one-shot test session](#1-run-the-basic-one-shot-test-session)
- [Choose the exact Postgres target](#2-pin-the-postgres-target)
- [Prepare migrations and other prerequisites](#3-prepare-the-repository-before-the-session)
- [Handle Django, pytest, and environment aliases](#4-adapt-the-application-to-the-created-database)
- [Choose a cleanup policy](#5-choose-cleanup-from-the-debugging-need)
- [Read structured session results](#6-make-the-result-machine-readable)
- [Decide between one suite and several partitions](#7-partition-for-a-reason-not-by-reflex)
- [Troubleshoot failed sessions](#troubleshooting-disposable-postgres-test-sessions)

## Prerequisites

Install and configure PGSandbox using the [setup guide](/docs/install/), make sure the selected managed PostgreSQL major or configured profile is available, and run `pgsandbox doctor` before the first session. Run the examples from the repository root unless you pass `--repo-path` explicitly.

Set the sandbox TTL longer than the command timeout. Leave enough additional time for provisioning, cleanup, and any deliberate failure inspection. The primary complete-session examples below use a 30-minute TTL with a 15-minute command timeout; shorter snippets omit repeated flags for readability.

## Disposable Postgres test workflow at a glance

Use this sequence:

1. Prepare non-database dependencies and build artifacts.
2. Select the PostgreSQL profile or major version and required extensions.
3. Start `pgsandbox with-database` with a bounded timeout and cleanup policy.
4. Let PGSandbox inject the sandbox connection into the child process.
5. Run the repository's actual migration and test entrypoint.
6. Record the structured status, child exit code, elapsed time, and cleanup outcome.
7. Inspect a retained failure or confirm deletion.

PostgreSQL itself distinguishes tests against an installed server from tests using a temporary installation in its [regression-test documentation](https://www.postgresql.org/docs/current/regress-run.html). PGSandbox takes a narrower approach: it uses an existing local or private PostgreSQL server, but gives each test session a fresh database and role on that server. It does not install a new server for every test run.

## The Session Proof Contract

A useful integration-test result answers five questions:

| Field | Question | Evidence |
| --- | --- | --- |
| Target | Which PostgreSQL environment received the test? | Profile, major version, requested extensions, safe database ID |
| Command | What did the repository actually run? | Direct executable plus argument list |
| Result | Did provisioning, process startup, or the tests fail? | Session status, child exit code, bounded stdout and stderr |
| Retention | Is a failed database still available for inspection? | Cleanup policy, retained flag, expiry time |
| Cleanup | Was the database deleted, already absent, or left after a cleanup failure? | Structured cleanup result |

The split matters because "tests failed" is not one condition. Provisioning can fail before the command starts. The executable can be missing. The child can return a failing exit code. The timeout can terminate the process. Tests can pass while cleanup fails. Each outcome needs a different recovery step.

The [PGSandbox MCP tool contract](/docs/mcp-tools/) covers the underlying database lifecycle operations. `with-database` is intentionally a CLI session wrapper because its job is to supervise a local child process and preserve process exit behavior.

Concurrency failures need that child-process boundary too. The [Postgres deadlock testing guide](/blog/test-postgres-deadlocks-lock-timeouts/) shows how one repository test process can hold two independent connections, coordinate opposite lock order, assert `40P01` and `55P03` separately, and leave cleanup to the enclosing disposable session.

## 1. Run the basic one-shot test session

Run a direct executable after `--`:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 30 \
  --cleanup always \
  --timeout-seconds 900 \
  -- python -m pytest tests/integration
```

PGSandbox creates a sandbox, then injects `DATABASE_URL`, `PGSANDBOX_DATABASE_URL`, and the libpq variables `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, and `PGDATABASE`. PostgreSQL documents how these variables map to connection parameters in its current [libpq environment-variable reference](https://www.postgresql.org/docs/current/libpq-envars.html).

The child command remains the repository's command. PGSandbox does not infer whether `pytest`, `cargo test`, `npm test`, or `make test` is correct. Keeping the executable and arguments explicit makes the run reviewable and avoids a shell string that can hide extra behavior.

On normal completion, the child exit code remains the command exit code. A timeout exits `124`. SIGINT and SIGTERM are translated to the conventional `128 + signal` status after PGSandbox terminates the child process group and applies cleanup. The public [`with-database` session documentation](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/540cb5653460b345c3382d26465de54a8670666f/docs/agent-testing.md) defines these fields and outcomes. Those semantics let CI and an agent distinguish an assertion failure from an interrupted or timed-out session.

## 2. Pin the Postgres target

Choose the environment that matches the compatibility question. Use a configured profile when the test depends on profile-specific policy or connectivity:

```bash
pgsandbox with-database \
  --profile team-pg17 \
  --cleanup always \
  -- make test-integration
```

Use `--postgres-version` for a managed local major:

```bash
pgsandbox with-database \
  --postgres-version 16 \
  --cleanup always \
  -- cargo test --test postgres_integration
```

Request required extensions explicitly and repeat the flag:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 30 \
  --extension vector \
  --extension pg_trgm \
  --cleanup on-success \
  -- uv run python -m pytest tests/search
```

The selected profile must allow each extension and the server must expose its supporting files. The [extension-testing guide](/blog/test-postgres-extensions-locally/) explains how to prove availability, registration, behavior, compatibility, and cleanup separately. A provisioning failure here should stop the test command; silently running without a required extension would test a different application state.

The sandbox role does not receive `CREATEDB` or superuser authority. The [PGSandbox architecture](/docs/architecture/) keeps lifecycle authority on the administration side and task SQL on the sandbox-role side.

## 3. Prepare the repository before the session

PGSandbox owns the disposable Postgres lifecycle, not every service in the test environment. Prepare Redis, object storage, browser drivers, generated assets, and frontend bundles before starting the database session.

For example:

```bash
npm ci
npm run build
pgsandbox with-database \
  --postgres-version 18 \
  --cleanup always \
  -- make test
```

This boundary prevented a misleading conclusion in PGSandbox's own [2026-07-21 test-session measurements](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/540cb5653460b345c3382d26465de54a8670666f/docs/session-benchmarks.md). A 1,173-test Rowset run first reproduced one failure because a frontend build artifact was missing. After the documented consumer prerequisite was built, the same monolithic suite passed. The sandbox lifecycle was working in both runs; the repository was not equally prepared.

Run migrations inside the child command or its test harness so they use the injected sandbox connection. If migration behavior is the subject of the change, capture schema and data evidence using the dedicated [database migration testing workflow](/blog/database-migration-testing-agent-pr/) rather than treating a green test summary as complete migration proof.

Prefer a repository-owned direct entrypoint for multi-step setup. For example, define `make verify-db` to run migrations, seed the minimum fixture, and then run the integration suite:

```makefile
.PHONY: verify-db
verify-db:
	python -m app.migrate
	python -m app.seed_test_data
	python -m pytest tests/integration
```

Then pass the direct executable and target:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 30 \
  --cleanup always \
  --timeout-seconds 900 \
  -- make verify-db
```

This keeps the workflow in version-controlled repository tooling. Do not hide a migration/test pipeline inside `sh -c` or `bash -c`; use a direct command with explicit arguments.

## 4. Adapt the application to the created database

Most clients will honor `DATABASE_URL` or the injected libpq variables directly. If an application expects another variable, add a repeatable alias:

```bash
pgsandbox with-database \
  --database-url-env DATABASE_URI \
  --database-url-env TEST_DATABASE_URL \
  --cleanup always \
  -- npm run test:integration
```

For a child running in Docker while PGSandbox runs on the host, use `--connection-mode local-container`. That mode rewrites a loopback host to `host.docker.internal`; Compose must still pass the injected variable into the service. Native Linux may also need a `host-gateway` mapping, and PostgreSQL still needs the correct listener and HBA rules. Non-loopback profiles are not rewritten. The [Docker-to-host Postgres guide](/blog/docker-connect-host-postgres/) covers those routing, listener, HBA, and secret boundaries.

Django needs one additional decision. Its normal test runner creates a separate `test_...` database and expects the configured role to have database-creation authority. PGSandbox roles intentionally lack `CREATEDB`. A repository adapter should tell Django to use the already-created sandbox and run migrations there. Keep that adapter in the consuming repository because settings modules, pytest fixtures, migration commands, and parallel-test behavior belong to the application.

When a pytest plugin must be importable from the checkout, prefer module invocation:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --cleanup always \
  -- uv run python -m pytest apps/core/tests/test_signals.py
```

Do not print the injected connection variables. PGSandbox redacts its generated credentials from captured child output, but application logs should still treat database URLs and passwords as secrets.

## 5. Choose cleanup from the debugging need

The cleanup policy is part of the test design:

| Policy | Passing command | Failing command | Best use |
| --- | --- | --- | --- |
| `always` | Delete | Delete | CI and unattended agent loops |
| `on-success` | Delete | Retain until manual deletion or TTL | Active debugging |
| `keep` | Retain | Retain | Deliberate inspection or benchmark work |

Use `always` when nobody will inspect the database interactively. Use `on-success` when a failure is more useful with its database state preserved. Use `keep` only when the retained state is the point of the run.

A retained sandbox remains bounded by its TTL and reports a safe database ID and expiry. TTL is the recovery path, not the normal cleanup path. The [sandbox TTL guide](/blog/postgres-sandbox-ttl-values/) shows how to combine expected runtime, review buffer, and recovery margin instead of copying one large timeout everywhere.

After inspecting a failure, close the loop with tracked deletion:

```bash
pgsandbox describe-schema --database-id <database-id>
pgsandbox delete-database --database-id <database-id>
```

Use the safe `databaseId` from the session result. Retrieve credentials only through the normal PGSandbox connection workflow and keep them out of logs, issues, and PR notes.

## 6. Make the result machine-readable

Add `--result-format json` when an agent or CI step needs stable fields:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 40 \
  --cleanup on-success \
  --timeout-seconds 1200 \
  --result-format json \
  -- make test-integration
```

The version 1 result reports a status such as `succeeded`, `provision-failed`, `child-spawn-failed`, `child-failed`, `timed-out`, `interrupted`, `cleanup-failed`, or `retained`. It also includes the safe sandbox identity, requested extensions, command exit and timing data, bounded redacted output, expiry, and cleanup state. PGSandbox does not place credentials in structured fields and redacts the exact generated URL and password from captured output. Child processes must still avoid logging transformed URLs or unrelated secrets.

In CI, preserve the JSON artifact and the CLI exit code separately:

```bash
set +e
pgsandbox with-database \
  --postgres-version 18 \
  --ttl-minutes 40 \
  --cleanup always \
  --timeout-seconds 1200 \
  --result-format json \
  -- make test-integration > pgsandbox-session.json
session_exit=$?
set -e

# Upload pgsandbox-session.json with your CI artifact mechanism.
exit "$session_exit"
```

Use the status before reading the child output:

- `provision-failed`: fix the profile, version, extension policy, or Postgres connection. The child never ran.
- `child-spawn-failed`: fix the repository path or executable. No test process started.
- `child-failed`: read the bounded output and inspect the retained sandbox when policy allows it.
- `timed-out` or `interrupted`: determine whether partial application state is useful, then confirm cleanup.
- `cleanup-failed`: preserve the reported sandbox identity and retry tracked deletion.

This status-first branch is more reliable than scanning prose for words such as "failed" or assuming every nonzero result came from the test framework.

## 7. Partition for a reason, not by reflex

One sandbox per entire suite minimizes repeated provisioning, migrations, and process startup. Multiple fresh sandboxes isolate failures and can support parallel CI, but each partition pays setup cost again.

PGSandbox's [2026-07-21 session measurements](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/540cb5653460b345c3382d26465de54a8670666f/docs/session-benchmarks.md) make that tradeoff concrete. On the same host, a prepared monolithic Rowset run passed 1,173 tests in 65.39 seconds wall time. Six sequential fresh-sandbox partitions passed the same 1,173 tests in 96.50 seconds, about 48% slower. The highest observed memory use across the partitions was about 9% lower than the monolithic run, and a failure would have been isolated to a smaller database scope.

Those numbers are workload-specific, not a general benchmark promise. They support a decision rule:

- Prefer one session when the repository is prepared, the suite fits available memory, and tests already isolate their state.
- Partition by stable boundaries when you need failure isolation, lower peak memory, parallel CI, or protection from process-level state accumulation.
- Measure both shapes on the same runner before claiming one is faster or more reliable.

Templates are also an optimization, not the correctness default. In the [same measurement record](https://github.com/LVTD-LLC/pgsandbox-mcp/blob/540cb5653460b345c3382d26465de54a8670666f/docs/session-benchmarks.md#template-yagni-gate), a synthetic 200-table, 200-index migration had a 2.457-second median fresh-create-plus-migration time and a 1.045-second median template-clone time. A template can repay its creation and invalidation cost for repeated migration-heavy sessions, but it must be keyed by every schema-affecting input. Start with fresh migrations until measurement shows a real bottleneck.

## Troubleshooting disposable Postgres test sessions

| Symptom | Likely boundary | Next action |
| --- | --- | --- |
| `provision-failed` | Target profile, version, extension, or admin connectivity | Run `pgsandbox doctor`; inspect the stable provisioning code |
| `child-spawn-failed` | Command or working directory | Check `--repo-path`, executable installation, and the direct argv |
| Application connects to the wrong database | Environment mapping | Confirm which variable the application reads; add `--database-url-env` if needed |
| Django tries to create `test_...` | Framework adapter | Configure the suite to use the injected database instead of requiring `CREATEDB` |
| Tests fail after a missing Redis or asset warning | Consumer prerequisite | Prepare the non-Postgres dependency, then rerun |
| Exit `124` | Session timeout | Narrow or partition the suite, or set a measured larger timeout |
| Passing tests but `cleanup-failed` | Lifecycle cleanup | Retry `delete-database` with the safe database ID or let TTL cleanup retry |
| Failed database disappeared before inspection | Cleanup policy | Use `on-success` with a sufficient TTL for the next debugging run |

## PR-ready session proof packet

Record this compact result with a database-dependent change:

```text
Disposable Postgres integration test
- Target: profile=<profile>, PostgreSQL=<major>, extensions=<list>
- Command: <executable and args>
- Status: <session status>, child exit=<code>, elapsed=<duration>
- Tests: <passed/failed/skipped summary>
- Retention: policy=<always/on-success/keep>, retained=<yes/no>, expires=<time if retained>
- Cleanup: attempted=<yes/no>, deleted=<yes/no>, error=<stable code or none>
- Notes: <prepared dependencies, partition, or retained database inspection>
```

This is enough for a reviewer to tell what ran, where it ran, why it failed, and whether state remains. It does not expose the database URL.

## Frequently asked questions

### What is a disposable Postgres database for integration tests?

It is a short-lived database created for one test session, with its own role, known target version, expiry, and cleanup path. It uses real PostgreSQL behavior while preventing the test process from sharing mutable database state with unrelated work.

### Does PGSandbox start a new Postgres server for every test?

No. PGSandbox uses an existing local or private PostgreSQL server and creates a disposable database and role on it. Choose the profile or managed local major explicitly when compatibility matters.

### Which cleanup policy should CI use?

Use `always` for CI and unattended agent runs. Use `on-success` during debugging when a failed database should remain available until manual deletion or TTL expiry.

### Can Django use the injected sandbox database?

Yes, but the repository should adapt Django's test settings so the runner uses the existing injected database instead of trying to create a separate `test_...` database. The sandbox role intentionally lacks `CREATEDB`.

### Should every test partition get a separate database?

Only when the isolation or parallelism benefit repays repeated setup. Measure a monolithic session and stable partitions on the same runner. One database per suite is often faster; partitions make failures smaller and can reduce peak memory.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Run integration tests in a disposable Postgres database",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Prepare the repository", "text": "Build required assets and start non-Postgres dependencies before the database session."},
        {"@type": "HowToStep", "position": 2, "name": "Select PostgreSQL", "text": "Choose the target profile or managed local PostgreSQL major and request required extensions."},
        {"@type": "HowToStep", "position": 3, "name": "Start the session", "text": "Run pgsandbox with-database with a timeout, cleanup policy, and direct repository command."},
        {"@type": "HowToStep", "position": 4, "name": "Use the injected connection", "text": "Configure the application to read DATABASE_URL, libpq variables, or an explicit database URL alias."},
        {"@type": "HowToStep", "position": 5, "name": "Record the result", "text": "Capture the structured session status, child exit code, elapsed time, and bounded redacted output."},
        {"@type": "HowToStep", "position": 6, "name": "Confirm cleanup", "text": "Verify deletion or record the safe identity and expiry of a deliberately retained failed database."}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "What is a disposable Postgres database for integration tests?", "acceptedAnswer": {"@type": "Answer", "text": "It is a short-lived database created for one test session, with its own role, known PostgreSQL target, expiry, and cleanup path."}},
        {"@type": "Question", "name": "Does PGSandbox start a new Postgres server for every test?", "acceptedAnswer": {"@type": "Answer", "text": "No. PGSandbox uses an existing local or private PostgreSQL server and creates a disposable database and role on it."}},
        {"@type": "Question", "name": "Which cleanup policy should CI use?", "acceptedAnswer": {"@type": "Answer", "text": "Use always for CI and unattended agent runs. Use on-success during debugging when a failed database should be retained until deletion or TTL expiry."}},
        {"@type": "Question", "name": "Can Django use the injected sandbox database?", "acceptedAnswer": {"@type": "Answer", "text": "Yes. Configure the repository's test settings to use the existing injected database instead of asking the restricted sandbox role to create another test database."}},
        {"@type": "Question", "name": "Should every test partition get a separate database?", "acceptedAnswer": {"@type": "Answer", "text": "Only when failure isolation, memory reduction, or parallelism repays repeated provisioning, migrations, and process startup. Measure both shapes on the same runner."}}
      ]
    }
  ]
}
</script>
