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
the change is docs-only. These scripts delegate to Cargo.

When changing MCP behavior, update `rust-src/mcp.rs`, `rust-src/postgres.rs`,
tests, and `docs/mcp-tools.md` together so tool schemas, implementation, and
docs stay aligned.
