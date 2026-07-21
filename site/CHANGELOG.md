# Changelog

## 2026-07-21

- Added the Astro Markdown blog post "How to Test pg_stat_statements in Agent Sandboxes" with PostgreSQL primary-source citations, an observability-boundary testing framework, SEO ledger and link inventory updates, and inbound blog links.

## 2026-07-20

- Added the Astro Markdown blog post "How to Test PostgreSQL Extension Upgrades Safely" with primary-source citations, a two-lane upgrade comparison, SEO ledger and link inventory updates, and inbound blog links.

## 2026-07-19

- Added the Astro Markdown blog post "How to Test PostgreSQL Extensions in Disposable Sandboxes" with SEO ledger, link inventory, primary-source citations, and inbound blog links.

## 2026-07-18

- Added the Astro Markdown blog post "Connect a Docker Container to Host PostgreSQL Safely" with SEO ledger, link inventory, inbound blog links, and a complete migration of repo-authored site URLs to the production domain.

## 2026-07-17

- Added the Astro Markdown blog post "PostgreSQL ROLE vs USER for Agent Database Access" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-16

- Added the Astro Markdown blog post "Per-Sandbox Postgres Roles for Coding Agents" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-15

- Added the Astro Markdown blog post "Postgres Sandbox Quotas for Coding Agents" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-14

- Added the Astro Markdown blog post "Postgres Test Database Cleanup: Choosing Sandbox TTLs" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-13

- Added the Astro Markdown blog post "Owner and Label Policy for Shared PGSandbox Profiles" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-12

- Added the Astro Markdown blog post "cleanup_expired vs Manual Postgres Cleanup for Agent Sandboxes" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-11
 
- Added the Astro Markdown blog post "How to Use cleanup_expired for Stale PGSandbox Resources" with SEO ledger, link inventory, and inbound blog links.

- Released `pgsandbox` v0.4.9 with more reliable agent repo workflows: database-id-first profile resolution, bounded stdin, Docker URL env aliases, explicit untracked-change semantics, redacted/filterable command output, clearer expired listings, concise smoke-test results, and preserved command output on timeout.

## 2026-07-10
- Improved agent repo workflows with database-id-first profile resolution,
  bounded stdin, Docker URL env aliases, explicit untracked-change semantics,
  redacted/filterable command output, clearer expired listings, and concise
  smoke-test results.
- Released `pgsandbox` v0.4.8 with the renamed `pgsandbox` CLI and package, CLI parity for the public MCP tools, safe uninstall support, and aligned GitHub release and Homebrew packaging.
- Added the Astro Markdown blog post "How to Use Local Postgres Versions with Coding Agents" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-09

- Added the Astro Markdown blog post "Postgres MCP Server Error Handling for Coding Agents" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-08

- Released `pgsandbox` v0.4.7 with actionable local Postgres install diagnostics, consistent managed-local profile resolution, restore compatibility classification for filtered clone archives, and structured clone timeout cleanup details.
- Added the Astro Markdown blog post "How to Run Agent SQL with Bounded Postgres Results" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-07

- Released `pgsandbox` v0.4.6 with the upgrade command, sandbox extension workflows, resolved target version fields, managed-local auto-install helpers, clone/restore compatibility fixes, extension discovery, and Docker connection variants.
- Released `pgsandbox` v0.4.5 with local Postgres discovery expanded through major version 13 and the new EXPLAIN plan agent SQL review guide.
- Added the Astro Markdown blog post "How to Use Postgres EXPLAIN Plans for Agent SQL Review" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-06

- Released `pgsandbox` v0.4.4 with simplified managed-local onboarding, scoped expired cleanup filters, typed multi-statement `run_sql` result sets, stricter readonly and row-limit validation, classified `explain_query` statement errors, clone-source repair hints, and clearer agent workflow docs.
- Released `pgsandbox` v0.4.3 with normalized MCP response envelopes, structured unknown-profile errors, encrypted sandbox role password migration, split schema relation descriptions, tighter repo command timeout hints, compact doctor version diagnostics, and updated ReviewGate setup guidance.
- Added the Astro Markdown blog post "How to Use Postgres Schema Snapshots for Agent Migration Reviews" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-05

- Released `pgsandbox` v0.4.2 with safer connection-string redaction, schema command timeout hardening, repo command validation polish, normalized `run_sql` int8 serialization, unsupported-type nullability handling, invalid TTL rejection, and simpler schema introspection payloads.
- Released `pgsandbox` v0.4.1 with the latest site content updates and Homebrew packaging metadata.
- Added the Astro Markdown blog post "Database Migration Testing Before Agent PRs" with SEO ledger, link inventory, and inbound blog links.
- Added the Astro Markdown blog post "Postgres Template Databases vs Task Sandboxes" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-03

- Released `pgsandbox` v0.4.0 with generic agent-first repo workflow tools, bounded schema validation and snapshot timeouts, MCP `doctor`, SQLSTATE-aware errors, safer connection-string redaction, richer `run_sql` metadata, cross-profile database-name lookup, and opt-in dogfood reliability coverage.
- Added the Astro Markdown blog post "Testcontainers vs Disposable Postgres Sandboxes for Agent Work" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-02

- Added the Astro Markdown blog post "How to Create a Postgres Test Database for Agent SQL" with SEO ledger, link inventory, and inbound blog links.

## 2026-07-01

- Released `pgsandbox` v0.3.0 with agent-safe cross-profile databaseId lookup, all-version list/cleanup modes, clone downgrade preflight, structured version diagnostics, and regression coverage for the agent-facing version contract.
- Released `pgsandbox` v0.2.1 with managed-local diagnostics, deferred MCP startup, short Unix socket paths, structured tool errors, and updated agent-facing docs.
- Released `pgsandbox` v0.2.0 with managed local multi-version Postgres support.
- Added the Astro Markdown blog post "What Is a Database Sandbox?" with SEO ledger, link inventory, and inbound blog links.
- Updated existing blog posts to link to the new database sandbox definition page.
- Added managed local multi-version Postgres runtime support with `--postgres-version`, `PGSANDBOX_POSTGRES_VERSION`, versioned `local-pg<major>` profiles, isolated data directories, and version-specific binary discovery.
- Added MCP `postgresVersion` selectors, `list_profiles`, and repo workflow inference from `.pgsandbox/project.json`, Compose, and devcontainer Postgres image tags.

## 2026-06-30

- Added the Docker-safe managed local Postgres runtime with init/start/stop/status helpers, free-port selection, health checks, and local profile persistence.
- Added lifecycle audit logging, local/private safety policy checks, allowed-host validation, per-owner sandbox quotas, and explicit external-admin opt-ins.
- Added repo workflow tools for preparing repos, running and validating migrations, seeding databases, and returning compact schema diffs.
- Added schema snapshot and local template workflows for repeatable agent database checkpoints, diffs, exports, restores, and cleanup.
- Expanded install, MCP client, no-Docker quickstart, troubleshooting, workflow, template, and snapshot documentation.
- Added the Astro Markdown blog post "How to Clone a Postgres Database Into a Safe Sandbox" with SEO ledger, link inventory, and inbound blog links.
- Updated the SEO foundation to reflect Astro Markdown blog files as the current source of truth.

## 2026-06-29

- Added a site build check for accessible Markdown table scroll wrappers on blog posts.
- Added a site content check that requires explicit blog post publication status.
- Moved the two published site blog posts out of Rowset and into Astro-managed Markdown content files.
- Added the database branching comparison brief and SEO ledger updates for the Rowset-backed blog.
- Added the initial SEO content foundation with brand guidance, Rowset blog configuration, internal link inventory, keyword research baseline, and candidate backlog for future PGSandbox blog posts.
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
- Added agent-assisted setup prompt guidance for installing, configuring, and verifying PGSandbox.
- Added a static Astro marketing/docs site under `site/` with a futuristic landing page, getting-started docs, Docker/CapRover packaging, and a GitHub Actions deployment workflow for main-branch updates.
- Renamed the product direction to PGSandbox and removed the default "experiment" framing from user-facing docs.
- Added AI steering files for product, technical, structure, vision, and design guidance.

## 2026-05-28

- Added MCP client setup commands for Codex, Cursor, VS Code, Claude Desktop, and all supported clients.
- Added a TypeScript/npm MCP server v0 with local Postgres profile support.
- Added database lifecycle, SQL execution, schema inspection, listing, and TTL cleanup tool implementations.
- Added CI, Cargo checks, and unit tests for configuration and name handling.
- Added initial repository scaffold for a local Postgres experimentation MCP.
- Documented v0 scope, MCP tool contract, architecture, safety rules, and local Postgres baseline.
