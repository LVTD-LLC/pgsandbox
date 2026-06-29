# SEO Content Ledger

## Shipped

| Date | Type | Title | Slug | Target Keyword | Primary Internal Links | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-06-29 | Guide / checklist | Postgres MCP Server Safety Checklist for Coding Agents | postgres-mcp-server-safety-checklist | postgres mcp server | /, /docs/mcp-tools/, /docs/architecture/ | Rowset row 1578, status `published`. |
| 2026-06-29 | Comparison | Database Branching vs Disposable Postgres Sandboxes | database-branching-vs-postgres-sandboxes | database branching | /, /docs/mcp-tools/, /docs/architecture/, /docs/install/, /blog/postgres-mcp-server-safety-checklist/ | Rowset row 1586, status `published`; compares environment-oriented branching with task-oriented sandboxes for agent workflows. |

## Removed

| Date | Title | Slug | Reason |
| --- | --- | --- | --- |
| 2026-06-29 | Disposable Postgres Databases for AI Agent Workflows | disposable-postgres-for-ai-agents | Rowset row removed by request. Do not link to this URL unless the post is restored. |

## Candidate Backlog

Last researched: 2026-06-29

| Rank | Score | Proposed Type | Title | Target Keyword | Volume | KD | Intent | SERP Read | Why It Fits |
| --- | ---: | --- | --- | --- | ---: | ---: | --- | --- | --- |
| 3 | 20 | How-to / tutorial | How to Clone a Postgres Database Into a Safe Disposable Sandbox | postgres clone database | 110 | 3 | Informational | SERP includes Stack Overflow, DBA StackExchange, Atlassian, pgcopydb, and practical pg_dump/pg_restore guides. | Very winnable and aligned with the next major product capability. Can use the repo's clone flow as first-hand implementation detail. |
| 4 | 18 | Definition / guide | What Is a Database Sandbox? | database sandbox | 90 | 15 | Informational | Results skew broad and mixed across vendor docs, online sandboxes, and educational material. | Good category-creation page, but slightly less direct than Postgres/MCP queries and broader intent may dilute conversion. |
| 5 | 16 | Tutorial | How to Create a Test Postgres Database for Agent-Generated SQL | postgres test database | 20 | 2 | Informational | Low-volume long-tail with practical developer intent. | Very winnable, but lower ceiling. Good future support piece for migration validation. |

## Notes

- Keyword data source: DataForSEO Google keyword overview and suggestions, United States / English, pulled 2026-06-29.
- SERP read source: web search fallback on 2026-06-29 because DataForSEO live SERP endpoint disconnected repeatedly from this environment.
- Treat `postgres mcp server` and `mcp postgres` as related but not identical: the former is broader informational discovery; the latter appears navigational and may skew toward specific registries/repos.
