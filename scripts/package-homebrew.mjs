import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";

const require = createRequire(import.meta.url);
const packageJson = require("../package.json");
const archive = `dist/pgsandbox-mcp-${packageJson.version}.tar.gz`;

const result = spawnSync(
  "tar",
  ["-czf", archive, "-C", "dist", "pgsandbox-mcp", "pgsandbox-mcp.cjs"],
  { stdio: "inherit" }
);

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const bytes = await readFile(archive);
const sha256 = createHash("sha256").update(bytes).digest("hex");

console.log(`archive: ${archive}`);
console.log(`sha256:  ${sha256}`);
