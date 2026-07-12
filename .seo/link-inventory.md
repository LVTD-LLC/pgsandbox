# Link Inventory

## Homepage / Core Marketing

| URL | Title / Target | Anchor Variants | Notes |
| --- | --- | --- | --- |
| / | PGSandbox MCP | PGSandbox MCP; disposable Postgres for coding agents; local MCP server for Postgres sandboxes | Primary product page. |
| https://github.com/LVTD-LLC/pgsandbox-mcp | GitHub repository | PGSandbox MCP on GitHub; GitHub repo; source code | External project proof. |

## Docs

| URL | Title / Target | Anchor Variants | Notes |
| --- | --- | --- | --- |
| /docs/ | PGSandbox MCP docs | docs; PGSandbox docs; installation and tool docs | Docs hub. |
| /docs/install/ | Install and setup | install PGSandbox MCP; setup guide; agent-assisted setup prompt | Strong install CTA. |
| /docs/mcp-tools/ | MCP tool contract | MCP tool contract; database lifecycle tools; create and clean up sandbox databases | Good internal link for tool-surface claims. |
| /docs/architecture/ | Architecture | architecture; resource model; scoped role and TTL model | Good internal link for safety/model claims. |
| /docs/homebrew/ | Homebrew packaging | Homebrew install; release packaging; Homebrew formula | Packaging/release topic. |

## Blog / Existing Content

| URL | Title / Target | Anchor Variants | Notes |
| --- | --- | --- | --- |
| /blog/ | Blog index | PGSandbox blog; database agent workflow posts; safe Postgres agent workflows | Blog hub. |
| /blog/postgres-mcp-server-safety-checklist/ | Postgres MCP Server Safety Checklist for Coding Agents | Postgres MCP server safety checklist; safe Postgres MCP server; MCP database safety checklist; database access checklist for coding agents | Rowset row 1578; status `published`. |
| /blog/database-branching-vs-postgres-sandboxes/ | Database Branching vs Disposable Postgres Sandboxes | database branching; database branching vs sandboxes; disposable Postgres sandboxes; agent database sandboxes | Rowset status `published`; comparison page for environment branches vs task sandboxes. |
| /blog/how-to-clone-postgres-database-sandbox/ | How to Clone a Postgres Database Into a Safe Sandbox | postgres clone database; clone Postgres into a sandbox; safe Postgres clone workflow; disposable Postgres clone; agent-safe database clone | How-to tutorial for cloning source state into a tracked PGSandbox destination. |
| /blog/what-is-database-sandbox/ | What Is a Database Sandbox? | database sandbox; what is a database sandbox; Postgres database sandbox; task-scoped database sandbox; database sandbox for coding agents | Definition and guide for database sandbox patterns, boundaries, and the five-part sandbox contract. |
| /blog/how-to-create-postgres-test-database-agent-sql/ | How to Create a Postgres Test Database for Agent SQL | postgres test database; Postgres test database for agent SQL; agent-generated SQL proof loop; task-scoped Postgres test database; SQL validation database for coding agents | How-to tutorial for creating a task database, applying schema state, running bounded generated SQL, and cleaning up. |
| /blog/testcontainers-vs-disposable-postgres-sandboxes/ | Testcontainers vs Disposable Postgres Sandboxes for Agent Work | Testcontainers vs disposable Postgres sandboxes; Testcontainers Postgres; disposable Postgres sandboxes; service container vs task database; Postgres sandbox for agent work | Comparison for choosing between Testcontainers service-container isolation and PGSandbox task-database isolation. |
| /blog/database-migration-testing-agent-pr/ | Database Migration Testing Before Agent PRs | database migration testing; Postgres migration validation; validate migrations before an agent PR; agent migration proof loop; migration schema diff for coding agents | How-to tutorial for validating Postgres migrations in a disposable sandbox before an agent opens a PR. |
| /blog/postgres-template-database-vs-task-sandbox/ | Postgres Template Databases vs Task Sandboxes | postgres template database; Postgres template database vs sandbox; task sandbox; reusable seeded sandbox; PGSandbox local template | Comparison for choosing between native Postgres templates, PGSandbox local template artifacts, and task-scoped sandboxes. |
| /blog/postgres-schema-snapshots-agent-migration-reviews/ | How to Use Postgres Schema Snapshots for Agent Migration Reviews | postgres schema snapshots; postgres schema diff; schema snapshots for migration review; agent migration schema diff; before and after schema checkpoint | How-to tutorial for using named schema snapshots and compact diffs as agent migration review evidence. |
| /blog/postgres-explain-plan-agent-sql/ | How to Use Postgres EXPLAIN Plans for Agent SQL Review | Postgres EXPLAIN plan for agent SQL; explain_query workflow; agent SQL plan review; query plan evidence for coding agents; EXPLAIN JSON sandbox review | How-to tutorial for using non-executing EXPLAIN plans, bounded SQL proof, and cleanup as an agent SQL review contract. |
| /blog/postgres-run-sql-bounded-results/ | How to Run Agent SQL with Bounded Postgres Results | bounded Postgres results; run agent SQL safely; run_sql bounded proof; Postgres rowLimit workflow; agent SQL result envelope | How-to tutorial for using `run_sql`, readonly mode, row limits, typed result sets, and PR-ready SQL proof summaries. |
| /blog/how-to-use-cleanup-expired-for-stale-pgsandbox-resources/ | How to Use cleanup_expired for Stale PGSandbox Resources | cleanup_expired for stale resources; stale sandbox cleanup; expired PGSandbox sandboxes; TTL cleanup workflow; metadata-scoped database cleanup | How-to tutorial for dry-run cleanup, scoped filters, and failure-aware cross-version cleanup in PGSandbox. |
| /blog/postgres-mcp-server-error-handling-coding-agents/ | Postgres MCP Server Error Handling for Coding Agents | Postgres MCP server error handling; MCP error handling for coding agents; stable Postgres MCP errors; SQLSTATE remediation envelope; agent database error runbook | Guide/checklist for branching on stable `errors[].code`, `category`, SQLSTATE, hints, and diagnostic handles instead of retrying blindly. |
| /blog/how-to-use-local-postgres-versions-with-coding-agents/ | How to Use Local Postgres Versions with Coding Agents | local postgres version; postgres version selection for agents; postgresVersion vs profile; managed local Postgres profiles; coding agent Postgres compatibility checks | How-to tutorial for selecting Postgres majors in PGSandbox, choosing between `postgresVersion` and `profile`, and recovery workflows for version/matching and local runtime errors. |
| /blog/cleanup-expired-vs-manual-postgres-cleanup/ | cleanup_expired vs Manual Postgres Cleanup for Agent Sandboxes | cleanup_expired vs manual Postgres cleanup; manual Postgres cleanup; metadata-backed cleanup; stale agent sandbox cleanup; Postgres role cleanup for agent sandboxes | Comparison for deciding when to use PGSandbox metadata-backed cleanup versus human-reviewed PostgreSQL cleanup commands. |

## Changelog

| URL | Title / Target | Anchor Variants | Notes |
| --- | --- | --- | --- |
| /changelog/ | Changelog | changelog; release notes; project updates | Useful for version/release references. |
