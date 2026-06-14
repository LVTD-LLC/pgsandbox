# Changelog

## Unreleased

- Added release automation to open Homebrew tap update PRs with versioned release asset URLs and SHA256 checksums.
- Added a static Astro marketing/docs site under `site/` with a futuristic landing page, getting-started docs, Docker/CapRover packaging, and a GitHub Actions deployment workflow for main-branch updates.
- Renamed the product direction to PGSandbox MCP and removed the default "experiment" framing from user-facing docs.
- Added a Rust native MCP server v0 with local Postgres profile support.
- Added database lifecycle, SQL execution, schema inspection, listing, and TTL cleanup tool implementations.
- Added CI, Cargo checks, and unit tests for configuration and name handling.
- Added initial repository scaffold for a local Postgres experimentation MCP.
- Documented v0 scope, MCP tool contract, architecture, safety rules, and local Postgres baseline.
