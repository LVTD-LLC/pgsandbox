# Changelog

## 2026-06-29

- Added a site build check for accessible Markdown table scroll wrappers on blog posts.
- Moved the two published site blog posts out of Rowset and into Astro-managed Markdown content files.
- Added the database branching comparison brief and SEO ledger updates for the Rowset-backed blog.
- Added the initial SEO content foundation with brand guidance, Rowset blog configuration, internal link inventory, keyword research baseline, and candidate backlog for future PGSandbox MCP blog posts.
- Updated the site deployment path so Rowset-backed blog pages are built in GitHub Actions before CapRover serves the static output.

## 2026-06-19

- Hardened the Astro site build by pinning Node 24.15.0 and upgrading to Astro 6 with explicit `overrides` for patched transitive build tooling: `esbuild`, `volar-service-yaml`, `yaml-language-server`, and `yaml`.
- Improved site navigation accessibility with 44px minimum interactive targets, safe-area viewport handling, current-page state on the home link, and location state for the top-level docs link on nested docs pages.

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
