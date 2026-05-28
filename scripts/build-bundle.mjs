import { chmod, writeFile } from "node:fs/promises";
import { build } from "esbuild";

await build({
  entryPoints: ["src/index.ts"],
  outfile: "dist/pgsandbox-mcp.cjs",
  bundle: true,
  platform: "node",
  target: "node20",
  format: "cjs",
  sourcemap: true,
  external: ["pg-native"]
});

await writeFile(
  "dist/pgsandbox-mcp",
  `#!/bin/sh
SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
exec node "$SCRIPT_DIR/pgsandbox-mcp.cjs" "$@"
`,
  "utf8"
);

await chmod("dist/pgsandbox-mcp", 0o755);
