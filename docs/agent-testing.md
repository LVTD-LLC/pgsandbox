# Agent Test-Suite Recipes

Use `with-database` when a repository test command needs a real, isolated
Postgres database but should not own its lifecycle.

## Generic one-shot run

Run a direct executable and pass its arguments after `--`:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --cleanup always \
  -- python -m pytest tests/unit
```

PGSandbox creates a restricted sandbox role, injects `DATABASE_URL`,
`PGSANDBOX_DATABASE_URL`, `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, and
`PGDATABASE`, captures bounded output for credential redaction, and deletes the
sandbox. The child exit code is the command exit code. A timeout exits `124`;
SIGINT and SIGTERM exit with `128 + signal` after terminating the child process
group and applying cleanup.

Use `--result-format json` when an agent needs stable fields instead of prose.
The version 1 result distinguishes provisioning, child, timeout, interruption,
retention, and cleanup outcomes. It includes safe sandbox identity, selected
Postgres version, requested extensions, elapsed time, bounded redacted output,
expiry, and cleanup state. It never includes a connection string or password.

### Version 1 JSON contract

`with-database` is CLI-only. Its JSON is a session result, not the shared MCP
tool envelope.

| Field | Shape | Meaning |
| --- | --- | --- |
| `schemaVersion` | integer | Always `1` for this contract. |
| `status` | string | `provision-failed`, `child-spawn-failed`, `child-failed`, `timed-out`, `interrupted`, `cleanup-failed`, `retained`, or `succeeded`. |
| `sandbox` | object or null | Safe `databaseId`, `databaseName`, `profile`, `postgresVersion`, and `expiresAt`; null when provisioning never produced a sandbox. |
| `requestedExtensions` | string array | Normalized extensions requested for the session. |
| `command` | object or null | `exitCode`, `signal`, `timedOut`, `elapsedMs`, redacted `stdout`/`stderr`, and their truncation flags; null when the child never started. |
| `cleanup` | object | `policy`, `attempted`, `deleted`, `retained`, `alreadyAbsent`, and nullable `errorCode`. Policies are `always`, `on-success`, and `keep`. |
| `provisionErrorCode` | string, omitted on success | Stable provisioning category without raw driver text. |
| `commandErrorCode` | string, omitted when command execution produced a result | `repo_not_found`, `unclear_command`, `invalid_env_alias`, `too_many_env_aliases`, `invalid_connection_mode`, or `child_spawn_failed`. |

Optional error-code fields are omitted rather than serialized as null. Nullable
fields inside `sandbox`, `command`, and `cleanup` remain present so consumers
can distinguish an absent value from an omitted phase.

The command accepts `--profile`, `--postgres-version`, `--name-hint`,
`--ttl-minutes`, `--owner`, repeated `--extension`, `--cleanup`,
`--timeout-seconds`, repeated `--database-url-env`, `--connection-mode`,
`--result-format`, and `--repo-path`. Run `pgsandbox --help` for defaults and
the accepted values.

## Extensions and cleanup

Request each required extension explicitly:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --extension vector \
  --extension pg_trgm \
  --cleanup on-success \
  -- python -m pytest tests/search
```

The selected profile must allow every requested extension. A denied extension
fails before privileged SQL; an unavailable extension fails provisioning and
rolls back partial resources. `on-success` deletes a passing database and keeps
a failing one for inspection. `keep` retains every database. Retained databases
remain bounded by their TTL, and the result reports their safe ID and expiry.

Use `always` in CI and unattended agent loops. Use `on-success` during active
debugging, then delete the retained sandbox when finished.

## Django and pytest

Django normally creates a separate `test_...` database and therefore expects
the application role to have `CREATEDB`. PGSandbox roles intentionally do not
have that privilege. A repository adapter should tell Django to use the already
created sandbox and run migrations there. Keep that hook in the consuming
repository because settings layout, migration commands, pytest fixtures, and
parallel-test behavior are framework and project decisions.

Prefer module invocation when a repository pytest plugin must be importable
from the checkout:

```bash
pgsandbox with-database \
  --postgres-version 18 \
  --extension vector \
  --cleanup always \
  -- uv run python -m pytest apps/core/tests/test_signals.py
```

`python -m pytest` places the current checkout on Python's import path in cases
where a standalone `pytest` entrypoint does not. This is a Python packaging
detail, not behavior PGSandbox should emulate.

## Other services and build prerequisites

PGSandbox owns only the disposable Postgres lifecycle. Redis, object storage,
browser drivers, frontend assets, and other services remain consumer
prerequisites. For example, a suite may use a developer Redis at
`127.0.0.1:6379`; verify or start it before the run rather than expecting
PGSandbox to provision it.

Likewise, run repository build prerequisites explicitly:

```bash
npm ci
npm run build
pgsandbox with-database --postgres-version 18 --cleanup always -- make test
```

## Troubleshooting

- `provision-failed`: inspect `provisionErrorCode`, profile selection, extension
  policy, and `pgsandbox doctor`; no child command ran.
- `child-spawn-failed`: check `--repo-path` and whether the direct executable is
  installed and on `PATH`.
- `child-failed`: inspect the bounded `stdout` and `stderr`; with
  `--cleanup on-success`, use the returned sandbox ID before its expiry.
- `cleanup-failed`: the result retains the sandbox identity and expiry. Retry
  `pgsandbox delete-database --database-id <id>` or let TTL cleanup retry.
- Missing Redis or assets: prepare those repository dependencies separately;
  they are outside PGSandbox's framework-neutral Postgres boundary.

Do not print the injected connection variables from application code. PGSandbox
redacts its generated values from captured output, but tests should still avoid
logging credentials as a general practice.

See [Session and template benchmarks](session-benchmarks.md) for measured
guidance on fresh migrations, optional templates, monolithic suites, and
fresh-sandbox partitions.
