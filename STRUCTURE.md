# PGSandbox MCP Repository Structure

## Top-Level Map

```text
.
|-- src/                         TypeScript source
|   |-- index.ts                 CLI entrypoint and command routing
|   |-- server.ts                MCP tool registration
|   |-- config.ts                env/JSON config loading
|   |-- postgres.ts              Postgres sandbox manager
|   |-- names.ts                 safe names and SQL quoting
|   |-- doctor.ts                diagnostics
|   |-- setup/client-config.ts   MCP client config writers
|   |-- version.ts               package version export
|   `-- *.test.ts                Vitest unit tests beside source
|-- docs/                        Architecture, install, MCP, release docs
|-- scripts/                     Build, clean, and packaging scripts
|-- .github/workflows/ci.yml     CI command sequence
|-- docker-compose.example.yml   Optional local demo Postgres
|-- README.md                    Primary user-facing guide
|-- package.json                 package metadata and scripts
`-- tsconfig.json                TypeScript compiler settings
```

## Module Boundaries

- Keep command parsing and process exit behavior in `src/index.ts`.
- Keep MCP schema registration in `src/server.ts`; do not bury tool names or
  input schemas inside database code.
- Keep database lifecycle and SQL behavior in `src/postgres.ts`.
- Keep identifier normalization and SQL quoting in `src/names.ts`.
- Keep client-specific config file formats in `src/setup/client-config.ts`.
- Keep docs in `docs/` unless the content is essential to the first README scan.

## Placement Rules

- Put tests next to the source file they cover as `*.test.ts`.
- Put new CLI subcommands in `src/index.ts` only when they are part of the
  public command surface.
- Put new MCP tools in `src/server.ts` and add manager methods only when the
  behavior belongs to sandbox lifecycle or inspection.
- Put new distribution scripts in `scripts/`; scripts should be runnable from
  the repo root and avoid hidden global state.
- Put optional local development assets at the root only when they help a user
  run the package quickly.

## Import Rules

- Use explicit `.js` extensions in TypeScript relative imports because the
  project compiles as NodeNext ESM.
- Prefer type-only imports for shared types.
- Do not introduce path aliases unless the module graph becomes genuinely hard
  to read.

## Naming Rules

- Public MCP tool names are snake_case.
- TypeScript functions and variables are camelCase.
- Types and classes are PascalCase.
- Environment variables are `PGSANDBOX_*`.
- Generated Postgres identifiers must stay within Postgres' 63 byte identifier
  limit.

## Special Cases

- `docker-compose.example.yml` is an example only. Do not make runtime code
  depend on Docker.
- `dist/` is generated output and should not be edited by hand.
- The prior Astro website work lives on a separate branch, not current `main`.
  If that branch is in scope, keep site changes isolated from core MCP changes
  unless the user explicitly asks to merge them.
