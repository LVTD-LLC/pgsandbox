# PGSandbox MCP Design Steering

## Scope

Current `main` is a CLI/MCP package, so design work mostly means docs, terminal
output, command examples, and any future website or UI branch.

## Documentation Style

- Lead with what the user can do, then explain why.
- Prefer exact commands over abstract descriptions.
- Keep setup paths copyable and version-aware.
- Separate npm, npx, Homebrew, and development-from-repo flows.
- Call out safety constraints directly: admin URL, local/private use, TTLs, and
  cleanup behavior.
- Avoid promising automatic Postgres installation or currently available hosted
  infrastructure before that surface is designed and implemented.

## CLI Output Style

- Use short, plain status lines.
- Mask credentials in diagnostics.
- Include the affected client, scope, and path when writing MCP config.
- For failures, name the missing environment variable, config file, client, or
  Postgres connection that needs attention.
- Do not print full sandbox connection strings except from commands or MCP tools
  whose purpose is to return credentials.

## Website Or UI Branch Guidance

If work targets the Astro site branch, preserve the quieter developer-docs
direction established in the prior design pass:

- restrained product/docs palette
- strong code readability
- active nav and sidebar states
- visible focus and hover states
- responsive code panels without horizontal page overflow
- first viewport that reveals the product and hints at following content

Avoid generic AI-site styling: oversized gradient heroes, decorative orb
backgrounds, pill clouds, and rounded card-heavy layouts that make the tool feel
less concrete.

## Visual Priorities

- The product signal should be Postgres sandboxes for agents, not generic cloud
  infrastructure.
- Show commands, lifecycle, profiles, cloning, and tool tables before abstract
  diagrams.
- Use diagrams only when they clarify the lifecycle or backend boundary.
- If hosted platform work appears in docs or UI, keep it tied to concrete
  database workflows: create, clone, inspect, share, delete, and audit. Avoid
  positioning that makes the local product sound like a limited clone of another
  hosted database service.
