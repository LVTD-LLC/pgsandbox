# PGSandbox product truth

Verified against current main `00902a6` on 2026-09-20. Evidence details and observations live in private Rowset; this is publishable claim guidance.

| Claim | Supported wording | Dated primary source | Risk |
|---|---|---|---|
| Local runtime and installation | Managed local PostgreSQL by default; setup and ensure_postgres can install missing binaries through a supported package manager when available. Manual installation may still be necessary. Explicit external profiles are optional; no hosted database service. | README.md, Run Setup; docs/mcp-tools.md, ensure_postgres; rust-src/local.rs, ensure_started_with_optional_install; rust-src/cli.rs setup (2026-09-20) | High |
| Expired resource cleanup | TTL records expiry. An explicit cleanup_expired call or user-owned scheduler removes expired metadata-owned sandboxes; expiry alone is not a background deletion timer. | README.md, Expired Sandboxes Remain; rust-src/postgres.rs cleanup_expired; rust-src/mcp.rs cleanup_expired (2026-09-20) | High |
| Bounded SQL results | run_sql preserves row limits and truncation metadata; the limit applies independently to statements with result sets. | docs/mcp-tools.md run_sql; rust-src/postgres.rs (2026-09-20) | Medium |

Do not generalize package-manager support into guaranteed installation. Do not imply that PGSandbox is a hosted service or that TTL alone enforces immediate resource removal.

## Extension provisioning — verified 2026-09-21

| Claim | Supported wording | Dated primary source | Risk |
|---|---|---|---|
| Requested extensions | create_database and clone_database validate allowedExtensions, then use the profile admin connection to install available extensions in the target database. Task SQL uses restricted sandbox credentials. | README.md, Extension workflows; docs/mcp-tools.md, create_database; rust-src/postgres.rs, create_database_internal and install_extensions at adbe9a8 (2026-09-21) | High |
| Profile defaults | Managed-local defaults allow pgcrypto, pg_stat_statements, pg_trgm, uuid-ossp and vector. Explicit profiles require an operator-configured allowlist. | rust-src/config.rs; rust-src/postgres.rs, validate_allowed_extensions; docs/mcp-tools.md (2026-09-21) | High |
| Ownership boundary | Lifecycle-installed extensions remain owned by the lifecycle role; provisioning does not grant extension-management authority to the sandbox role. Direct SQL follows PostgreSQL privileges. | README.md, Security model; PostgreSQL CREATE EXTENSION documentation (2026-09-21) | High |

## Reader-facing editorial language — verified 2026-09-24

Published guides explain the technical benefit directly; they do not narrate internal editorial criteria. The scoped content rule covers the guides corrected in this pass. Technical examples, cited sources, and product claims are unchanged. Source: the maintained brand voice and the corrected guide paragraphs.

## Claude client setup — verified 2026-10-09

PGSandbox v0.5.0 writes Claude Desktop configuration with `setup --client claude-desktop`, user scope only. Its client selector does not accept `claude-code`. Claude Code can register the `pgsandbox mcp` stdio launch command through its own `claude mcp add` command. Source: `rust-src/setup.rs` at v0.5.0 and current main; https://code.claude.com/docs/en/mcp (checked 2026-10-09). This documents configuration compatibility, not a completed Claude Code end-to-end test.

## Comparison runtime boundary — verified 2026-10-10

Managed-local PostgreSQL is the default, not a separately configured external-host prerequisite. Setup prepares it through `ensure_setup_managed_local` and `ensure_started_with_optional_install(true)`; package-manager availability still conditions installation. External profiles are optional. TTL requires an explicit cleanup call or user-owned scheduler. Sources: `rust-src/cli.rs` setup flow and `rust-src/local.rs`, `README.md` at main `402dfa3` (2026-10-10).

## Extension-upgrade readiness — verified 2026-10-16

The runtime smoke test checks a disposable database lifecycle, not the availability or compatibility of a particular extension update path. Readers must verify the selected profile and extension package separately. TTL records expiry; explicit `cleanup_expired` or a user-owned scheduler performs expired cleanup. Sources: README.md verification and Expired Sandboxes Remain sections; docs/mcp-tools.md; rust-src/postgres.rs at main `448a487` (2026-10-16).

## Partial-index first-run boundary — verified 2026-10-18

Managed-local PostgreSQL is the default; an explicit external profile is optional. Versioned `doctor` and `smoke-test` checks are supported. A smoke test proves the runtime lifecycle, not migration correctness or partial-index eligibility. `with-database --cleanup always` requests post-child cleanup; inspect its result. TTL alone does not schedule deletion. Sources: README.md Manual Runtime Checks, rust-src/cli.rs setup and with-database flows, docs/mcp-tools.md at main `49a6fc8` (2026-10-18).

## Snapshot teardown order — verified 2026-10-19

`delete_schema_snapshot` resolves a PGSandbox-owned database record before deleting the local artifact. Remove the named snapshot while that sandbox is tracked, then delete the database and inspect the result. TTL alone does not schedule cleanup. Sources: rust-src/postgres.rs `delete_schema_snapshot` and `get_owned_record`, docs/mcp-tools.md snapshot contract at main `d12f848` (2026-10-19). Snapshot/diff requests are MCP client tool requests, not CLI commands or a proof of migration correctness.

## Integration-test session readiness and cleanup — verified 2026-10-20

Versioned `doctor` and `smoke-test` checks target the managed-local major; they do not prove an external profile or an application's test suite. `with-database` reports its cleanup outcome; a retained database's expiry does not itself delete it. Use explicit tracked deletion, or explicit `cleanup_expired`/a user-owned scheduler after expiry, and inspect the result. Sources: README.md runtime checks, docs/agent-testing.md, rust-src/cli.rs doctor/smoke-test and cleanup-expired dispatch, rust-src/postgres.rs cleanup_expired at main `405e45b` (2026-10-20).

## Clone workflow boundary — verified 2026-10-24

Current main `2aecc2f`, docs/mcp-tools.md and rust-src/postgres.rs: sourceDatabaseUrl names the source; profile/postgresVersion select the destination. schemaOnly omits table data, not schema definitions or comments; there are no masking or row-filter inputs. pg_dump and pg_restore must be on PATH; source major newer than target is rejected. TTL records expiry and explicit deletion or cleanup_expired removes tracked resources. PostgreSQL15 pg_restore documents that restoring source-defined code is not safe for untrusted sources, including partial restores. Source: https://www.postgresql.org/docs/15/app-pgrestore.html (read October24).
