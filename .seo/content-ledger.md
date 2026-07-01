# SEO Content Ledger

## Shipped

| Date | Type | Title | Slug | Target Keyword | Primary Internal Links | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-06-29 | Guide / checklist | Postgres MCP Server Safety Checklist for Coding Agents | postgres-mcp-server-safety-checklist | postgres mcp server | /, /docs/mcp-tools/, /docs/architecture/ | Rowset row 1578, status `published`. |
| 2026-06-29 | Comparison | Database Branching vs Disposable Postgres Sandboxes | database-branching-vs-postgres-sandboxes | database branching | /, /docs/mcp-tools/, /docs/architecture/, /docs/install/, /blog/postgres-mcp-server-safety-checklist/ | Rowset row 1586, status `published`; compares environment-oriented branching with task-oriented sandboxes for agent workflows. |
| 2026-06-30 | How-to / tutorial | How to Clone a Postgres Database Into a Safe Sandbox | how-to-clone-postgres-database-sandbox | postgres clone database | /docs/mcp-tools/, /docs/architecture/, /docs/install/, /blog/postgres-mcp-server-safety-checklist/, /blog/database-branching-vs-postgres-sandboxes/ | Astro Markdown source of truth; uses Postgres primary docs and repo clone implementation as information gain. |
| 2026-07-01 | Definition / guide | What Is a Database Sandbox? | what-is-database-sandbox | database sandbox | /docs/architecture/, /docs/mcp-tools/, /docs/install/, /blog/postgres-mcp-server-safety-checklist/, /blog/database-branching-vs-postgres-sandboxes/, /blog/how-to-clone-postgres-database-sandbox/ | Astro Markdown source of truth; uses the five-part sandbox contract as the information-gain framework. |

## Removed

| Date | Title | Slug | Reason |
| --- | --- | --- | --- |
| 2026-06-29 | Disposable Postgres Databases for AI Agent Workflows | disposable-postgres-for-ai-agents | Rowset row removed by request. Do not link to this URL unless the post is restored. |

## Candidate Backlog

Last researched: 2026-06-29

| Rank | Score | Proposed Type | Title | Target Keyword | Volume | KD | Intent | SERP Read | Why It Fits |
| --- | ---: | --- | --- | --- | ---: | ---: | --- | --- | --- |
| 5 | 16 | Tutorial | How to Create a Test Postgres Database for Agent-Generated SQL | postgres test database | 20 | 2 | Informational | Low-volume long-tail with practical developer intent. | Very winnable, but lower ceiling. Good future support piece for migration validation. |

## Notes

- Keyword data source: DataForSEO Google keyword overview and suggestions, United States / English, pulled 2026-06-29.
- SERP read source: web search fallback on 2026-06-29 because DataForSEO live SERP endpoint disconnected repeatedly from this environment.
- Treat `postgres mcp server` and `mcp postgres` as related but not identical: the former is broader informational discovery; the latter appears navigational and may skew toward specific registries/repos.
- 2026-06-30 cron selected the top remaining backlog candidate automatically per cron instruction. New source-of-truth content file: `site/src/content/blog/how-to-clone-postgres-database-sandbox.md`.
- 2026-07-01 cron selected the top remaining backlog candidate automatically per cron instruction. New source-of-truth content file: `site/src/content/blog/what-is-database-sandbox.md`.
