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
