# Claude Code Instructions

Read `AGENTS.md` first. The shared steering files are the source of truth:

- `PRODUCT.md`
- `TECH.md`
- `STRUCTURE.md`
- `VISION.md`
- `DESIGN.md`

Before editing, inspect `git status --short` and avoid overwriting unrelated
user changes. Use the repo commands from `AGENTS.md`; for implementation work,
run `npm run check`, `npm test`, and `npm run build` before final handoff unless
the change is docs-only.

When changing MCP behavior, update `src/server.ts`, `src/postgres.ts`, tests,
and `docs/mcp-tools.md` together so tool schemas, implementation, and docs stay
aligned.
