# PGSandbox MCP Repository Structure

## Top-Level Map

```text
.
|-- rust-src/                    Rust source
|   |-- main.rs                  Binary entrypoint
|   |-- cli.rs                   CLI command routing
|   |-- mcp.rs                   MCP tool registration
|   |-- config.rs                env/JSON config loading
|   |-- postgres.rs              Postgres sandbox manager
|   |-- names.rs                 safe names and SQL quoting
|   |-- doctor.rs                diagnostics
|   |-- setup.rs                 MCP client config writers
|   `-- lib.rs                   Library exports and package version
|-- docs/                        Architecture, install, MCP, release docs
|-- scripts/                     Build, clean, and packaging scripts
|-- .github/workflows/ci.yml     CI command sequence
|-- docker-compose.example.yml   Optional local demo Postgres
|-- README.md                    Primary user-facing guide
|-- Cargo.toml                   Rust package metadata
`-- package.json                 JavaScript helper scripts for site/packaging
```

## Module Boundaries

- Keep command parsing and process exit behavior in `rust-src/cli.rs`.
- Keep MCP schema registration in `rust-src/mcp.rs`; do not bury tool names or
  input schemas inside database code.
- Keep database lifecycle and SQL behavior in `rust-src/postgres.rs`.
- Keep identifier normalization and SQL quoting in `rust-src/names.rs`.
- Keep client-specific config file formats in `rust-src/setup.rs`.
- Keep docs in `docs/` unless the content is essential to the first README scan.

## Placement Rules

- Put unit tests in the same Rust source module they cover.
- Put new CLI subcommands in `rust-src/cli.rs` only when they are part of the
  public command surface.
- Put new MCP tools in `rust-src/mcp.rs` and add manager methods only when the
  behavior belongs to sandbox lifecycle or inspection.
- Put new distribution scripts in `scripts/`; scripts should be runnable from
  the repo root and avoid hidden global state.
- Put optional local development assets at the root only when they help a user
  run the package quickly.

## Import Rules

- Keep module exports explicit in `rust-src/lib.rs`.
- Prefer standard-library and existing crate functionality before adding new
  dependencies.
- Do not introduce broad helper modules unless the module graph becomes
  genuinely hard to read.

## Naming Rules

- Public MCP tool names are snake_case.
- Rust functions and variables are snake_case.
- Rust types and structs are PascalCase.
- Environment variables are `PGSANDBOX_*`.
- Generated Postgres identifiers must stay within Postgres' 63 byte identifier
  limit.

## Special Cases

- `docker-compose.example.yml` is an example only. Do not make runtime code
  depend on Docker.
- Release archives and generated site assets are build outputs and should not be
  edited by hand.
- The Astro website lives under `site/`. Keep site changes isolated from core
  MCP changes unless the user explicitly asks to update both.
