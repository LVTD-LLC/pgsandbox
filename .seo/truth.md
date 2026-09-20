# PGSandbox product truth

Verified against current main `00902a6` on 2026-09-20. Evidence details and observations live in private Rowset; this is publishable claim guidance.

| Claim | Supported wording | Dated primary source | Risk |
|---|---|---|---|
| Local runtime and installation | Managed local PostgreSQL by default; setup and ensure_postgres can install missing binaries through a supported package manager when available. Manual installation may still be necessary. Explicit external profiles are optional; no hosted database service. | README.md, Run Setup; docs/mcp-tools.md, ensure_postgres; rust-src/local.rs, ensure_started_with_optional_install; rust-src/cli.rs setup (2026-09-20) | High |
| Expired resource cleanup | TTL records expiry. An explicit cleanup_expired call or user-owned scheduler removes expired metadata-owned sandboxes; expiry alone is not a background deletion timer. | README.md, Expired Sandboxes Remain; rust-src/postgres.rs cleanup_expired; rust-src/mcp.rs cleanup_expired (2026-09-20) | High |
| Bounded SQL results | run_sql preserves row limits and truncation metadata; the limit applies independently to statements with result sets. | docs/mcp-tools.md run_sql; rust-src/postgres.rs (2026-09-20) | Medium |

Do not generalize package-manager support into guaranteed installation. Do not imply that PGSandbox is a hosted service or that TTL alone enforces immediate resource removal.
