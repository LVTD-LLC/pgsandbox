# SEO Content Ledger

## Shipped

| Date | Type | Title | Slug | Target Keyword | Primary Internal Links | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-06-29 | Guide / checklist | Postgres MCP Server Safety Checklist for Coding Agents | postgres-mcp-server-safety-checklist | postgres mcp server | /, /docs/mcp-tools/, /docs/architecture/ | Rowset row 1578, status `published`. |
| 2026-06-29 | Comparison | Database Branching vs Disposable Postgres Sandboxes | database-branching-vs-postgres-sandboxes | database branching | /, /docs/mcp-tools/, /docs/architecture/, /docs/install/, /blog/postgres-mcp-server-safety-checklist/ | Rowset row 1586, status `published`; compares environment-oriented branching with task-oriented sandboxes for agent workflows. |
| 2026-06-30 | How-to / tutorial | How to Clone a Postgres Database Into a Safe Sandbox | how-to-clone-postgres-database-sandbox | postgres clone database | /docs/mcp-tools/, /docs/architecture/, /docs/install/, /blog/postgres-mcp-server-safety-checklist/, /blog/database-branching-vs-postgres-sandboxes/ | Astro Markdown source of truth; uses Postgres primary docs and repo clone implementation as information gain. |
| 2026-07-01 | Definition / guide | What Is a Database Sandbox? | what-is-database-sandbox | database sandbox | /docs/architecture/, /docs/mcp-tools/, /docs/install/, /blog/postgres-mcp-server-safety-checklist/, /blog/database-branching-vs-postgres-sandboxes/, /blog/how-to-clone-postgres-database-sandbox/ | Astro Markdown source of truth; uses the five-part sandbox contract as the information-gain framework. |
| 2026-07-02 | How-to / tutorial | How to Create a Postgres Test Database for Agent SQL | how-to-create-postgres-test-database-agent-sql | postgres test database | /docs/mcp-tools/, /docs/architecture/, /blog/postgres-mcp-server-safety-checklist/, /blog/what-is-database-sandbox/, /blog/how-to-clone-postgres-database-sandbox/ | Astro Markdown source of truth; uses the agent-generated SQL proof harness as the information-gain framework. |
| 2026-07-03 | Comparison | Testcontainers vs Disposable Postgres Sandboxes for Agent Work | testcontainers-vs-disposable-postgres-sandboxes | testcontainers postgres | /docs/mcp-tools/, /docs/architecture/, /docs/install/, /blog/how-to-create-postgres-test-database-agent-sql/, /blog/what-is-database-sandbox/ | Astro Markdown source of truth; uses the service-container vs task-database authority boundary as the information-gain framework. |
| 2026-07-05 | How-to / tutorial | Database Migration Testing Before Agent PRs | database-migration-testing-agent-pr | database migration testing | /docs/mcp-tools/, /docs/architecture/, /blog/what-is-database-sandbox/, /blog/how-to-clone-postgres-database-sandbox/, /blog/testcontainers-vs-disposable-postgres-sandboxes/ | Astro Markdown source of truth; uses the PR-ready sandbox proof loop as the information-gain framework. |
| 2026-07-05 | Comparison / explainer | Postgres Template Databases vs Task Sandboxes | postgres-template-database-vs-task-sandbox | postgres template database | /docs/mcp-tools/, /docs/architecture/, /blog/how-to-create-postgres-test-database-agent-sql/, /blog/database-migration-testing-agent-pr/ | Astro Markdown source of truth; uses the native template vs PGSandbox local template artifact boundary as the information-gain framework. |
| 2026-07-06 | How-to / tutorial | How to Use Postgres Schema Snapshots for Agent Migration Reviews | postgres-schema-snapshots-agent-migration-reviews | postgres schema diff | /docs/mcp-tools/, /docs/architecture/, /docs/agent-workflows/, /blog/database-migration-testing-agent-pr/, /blog/what-is-database-sandbox/, /blog/how-to-clone-postgres-database-sandbox/ | Astro Markdown source of truth; uses the review-grade schema snapshot contract as the information-gain framework. |

## Removed

| Date | Title | Slug | Reason |
| --- | --- | --- | --- |
| 2026-06-29 | Disposable Postgres Databases for AI Agent Workflows | disposable-postgres-for-ai-agents | Rowset row removed by request. Do not link to this URL unless the post is restored. |

## Candidate Backlog

Last researched: 2026-07-05

| Rank | Score | Proposed Type | Title | Target Keyword | Volume | KD | Intent | SERP Read | Why It Fits |
| --- | ---: | --- | --- | --- | ---: | ---: | --- | --- | --- |
| 1 | 14 | Needs refresh | Next schema-diff adjacent opportunity | TBD | TBD | TBD | TBD | 2026-07-06 cron shipped the previous top backlog item. | Rebuild from live keyword/SERP data on the next run; DataForSEO credentials were unavailable in this environment. |

## Notes

- Keyword data source: DataForSEO Google keyword overview and suggestions, United States / English, pulled 2026-06-29.
- SERP read source: web search fallback on 2026-06-29 because DataForSEO live SERP endpoint disconnected repeatedly from this environment.
- 2026-07-02 backlog refresh used web-search fallback because DataForSEO credentials were not available in the cron environment. Treat `TBD` volume/KD rows as candidates requiring keyword-tool refresh before final selection.
- Treat `postgres mcp server` and `mcp postgres` as related but not identical: the former is broader informational discovery; the latter appears navigational and may skew toward specific registries/repos.
- 2026-06-30 cron selected the top remaining backlog candidate automatically per cron instruction. New source-of-truth content file: `site/src/content/blog/how-to-clone-postgres-database-sandbox.md`.
- 2026-07-01 cron selected the top remaining backlog candidate automatically per cron instruction. New source-of-truth content file: `site/src/content/blog/what-is-database-sandbox.md`.
- 2026-07-02 cron selected the top remaining backlog candidate automatically per cron instruction. New source-of-truth content file: `site/src/content/blog/how-to-create-postgres-test-database-agent-sql.md`.
- 2026-07-03 cron selected the top remaining backlog candidate automatically per cron instruction. New source-of-truth content file: `site/src/content/blog/testcontainers-vs-disposable-postgres-sandboxes.md`.
- 2026-07-05 cron selected the top remaining backlog candidate automatically per cron instruction. DataForSEO helper was installed but credentials were unavailable in the cron environment, so live keyword volume/KD stayed `TBD` and the run used web/primary-source fallback. New source-of-truth content file: `site/src/content/blog/database-migration-testing-agent-pr.md`.
- 2026-07-05 06:00 UTC cron selected the top remaining backlog candidate automatically per cron instruction. DataForSEO helper was installed but credentials were unavailable in the cron environment, so live keyword volume/KD stayed `TBD` and the run used web/primary-source fallback. New source-of-truth content file: `site/src/content/blog/postgres-template-database-vs-task-sandbox.md`.
- 2026-07-06 cron selected the top remaining backlog candidate automatically per cron instruction. DataForSEO helper was installed but `DATAFORSEO_LOGIN` / `DATAFORSEO_PASSWORD` were unavailable, so live keyword volume/KD stayed `TBD` and the run used web/primary-source fallback. New source-of-truth content file: `site/src/content/blog/postgres-schema-snapshots-agent-migration-reviews.md`.
