# Changelog

## 2026-06-14

- Added an auto-generated site changelog page sourced from `CHANGELOG.md` and linked it from the site footer.
- Added database cloning support with hardened clone errors and TLS parameter forwarding.
- Simplified the homepage call to action around the agent setup prompt and docs.
- Added a homepage flow for copying the agent-assisted setup prompt.
- Reduced setup prompt interaction so agents can proceed automatically when a safe local configuration is discoverable.

## 2026-06-13

- Added release automation to open Homebrew tap update PRs with versioned release asset URLs and SHA256 checksums.
- Added a GitHub release installer script for downloading and installing platform-specific binaries.
- Rewrote the MCP server as a Rust native binary with local Postgres profile support.
- Added agent-assisted setup prompt guidance for installing, configuring, and verifying PGSandbox MCP.
- Added a static Astro marketing/docs site under `site/` with a futuristic landing page, getting-started docs, Docker/CapRover packaging, and a GitHub Actions deployment workflow for main-branch updates.
- Renamed the product direction to PGSandbox MCP and removed the default "experiment" framing from user-facing docs.
- Added AI steering files for product, technical, structure, vision, and design guidance.

## 2026-05-28

- Added MCP client setup commands for Codex, Cursor, VS Code, Claude Desktop, and all supported clients.
- Added a TypeScript/npm MCP server v0 with local Postgres profile support.
- Added database lifecycle, SQL execution, schema inspection, listing, and TTL cleanup tool implementations.
- Added CI, Cargo checks, and unit tests for configuration and name handling.
- Added initial repository scaffold for a local Postgres experimentation MCP.
- Documented v0 scope, MCP tool contract, architecture, safety rules, and local Postgres baseline.
