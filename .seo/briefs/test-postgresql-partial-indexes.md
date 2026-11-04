# Named application-query proof — November 4, 2026

Existing owner: `/blog/test-postgresql-partial-indexes/`; related owner:
`/blog/postgres-explain-plan-agent-sql/`. No new route or title experiment.

## Information gain

A runnable named node-postgres query on one checked-out client separates
observed automatic plan selection from forced diagnostic controls. It checks
exact result sets and demonstrates that a fixed partial predicate remains
provable when another condition is parameterized. The one-shot session reports
child success separately from database deletion.

## Sources and scope

- node-postgres queries, Pool and Client APIs: https://node-postgres.com/features/queries,
  https://node-postgres.com/apis/pool, https://node-postgres.com/apis/client.
- PostgreSQL 15 PREPARE, partial indexes and session counters:
  https://www.postgresql.org/docs/15/sql-prepare.html,
  https://www.postgresql.org/docs/15/indexes-partial.html,
  https://www.postgresql.org/docs/15/view-pg-prepared-statements.html.
- PGSandbox session contract: `docs/agent-testing.md`, `rust-src/cli.rs`,
  `rust-src/postgres.rs` at source main `8992d9c`.
- Synthetic rehearsal: installed PGSandbox 0.5.0, pg 8.23.1, Node 24.22.1,
  PostgreSQL 15.19. Both the exploratory proof and the exact published JavaScript
  passed; the published one-shot command reported child exit 0 and deleted true.

This is not application/ORM coverage, an automatic-switch guarantee, a runtime
feature release, a security benchmark, or production-performance evidence.
Repeat against the real migration, driver, workload and server major.
No customer data, private analytics or database credentials appear in the copy.

## Content and maintenance

Keep original titles, metadata and publication dates; bump modification dates.
Link from the EXPLAIN owner to the new subsection. Keep existing installation,
hub and lifecycle links. Do not infer growth from successful execution.
Recheck driver/server syntax when updating the pinned example. Durable research,
measurement, action state and outcomes stay in the existing private Rowset store
identified by `.seo/config.json`, not in this brief.

The older Psycopg session example included unsupported `--env-var` and `--label`; removed both.
Both current CLI dispatch and `database_command_env` confirm that
`PGSANDBOX_DATABASE_URL` is injected by default. The new one-shot rehearsal
uses that default and verifies the child can connect.

A bounded duplicate check found unsupported flags in the foreign-key cascade,
two-phase-commit, deferrable-constraint and generated-column recipes. Corrected
only the one-shot blocks and modification dates; retained the two short purpose
names with supported `--name-hint`. All published bash `with-database` option
names were checked against current CLI dispatch. Existing SQL/Python harnesses
were not changed or re-executed.
