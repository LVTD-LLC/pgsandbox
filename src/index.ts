#!/usr/bin/env node
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { loadConfig } from "./config.js";
import { runDoctor } from "./doctor.js";
import { PostgresSandboxManager } from "./postgres.js";
import { createServer } from "./server.js";
import {
  buildLaunchConfig,
  configSnippet,
  resolveTargets,
  writeClientConfig,
  type ClientSelector,
  type ConfigScope,
} from "./setup/client-config.js";
import { VERSION } from "./version.js";

async function main() {
  const [command, ...args] = process.argv.slice(2);

  if (!command || command === "stdio") {
    await startServer();
    return;
  }

  if (command === "--help" || command === "-h" || command === "help") {
    printHelp();
    return;
  }

  if (command === "--version" || command === "-v" || command === "version") {
    console.log(VERSION);
    return;
  }

  if (command === "setup") {
    await setup(args);
    return;
  }

  if (command === "doctor") {
    await doctor(args);
    return;
  }

  if (command === "smoke-test") {
    await smokeTest(args);
    return;
  }

  throw new Error(`Unknown command: ${command}`);
}

async function startServer() {
  const config = loadConfig();
  const server = createServer(config);
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

async function setup(args: string[]) {
  const options = parseOptions(args);
  const client = parseClient(String(options.client ?? "codex"));
  const scope = parseScope(String(options.scope ?? "user"));
  const adminUrl = stringOption(options, "admin-url");
  const launch = buildLaunchConfig({
    name: stringOption(options, "name") ?? "pgsandbox",
    command: stringOption(options, "command") ?? "pgsandbox-mcp",
    adminUrl,
  });
  const targets = resolveTargets({ client, scope, cwd: process.cwd() });

  if (!adminUrl) {
    console.warn(
      "No PGSANDBOX_ADMIN_DATABASE_URL was written. The MCP client must provide it in the server environment.",
    );
  }

  for (const target of targets) {
    const result = await writeClientConfig(target, launch, Boolean(options["dry-run"]));
    console.log(`${result.action}: ${target.client} ${target.scope} ${target.path}`);
    if (options["dry-run"]) {
      console.log(configSnippet(target, launch));
    }
  }

  console.log("Next: restart the MCP client, then run `pgsandbox-mcp doctor`.");
}

async function doctor(args: string[]) {
  const options = parseOptions(args);
  const result = await runDoctor({
    adminUrl: stringOption(options, "admin-url"),
    cwd: process.cwd(),
  });

  for (const line of result.lines) {
    console.log(line);
  }

  process.exitCode = result.ok ? 0 : 1;
}

async function smokeTest(args: string[]) {
  const options = parseOptions(args);
  const config = loadConfig(
    options["admin-url"]
      ? { ...process.env, PGSANDBOX_ADMIN_DATABASE_URL: String(options["admin-url"]) }
      : process.env,
  );
  const manager = new PostgresSandboxManager(config);
  let databaseId: string | undefined;

  try {
    const created = await manager.createDatabase({
      nameHint: "smoke test",
      ttlMinutes: 15,
      owner: "smoke",
    });
    databaseId = created.databaseId;
    console.log(`Created sandbox: ${created.databaseName}`);

    const query = await manager.runSql({
      databaseId,
      sql: "select 1 as ok",
      readonly: true,
    });
    console.log(JSON.stringify(query, null, 2));

    await manager.deleteDatabase({ databaseId });
    console.log("Cleanup: deleted");
    databaseId = undefined;
  } finally {
    if (databaseId) {
      await manager.deleteDatabase({ databaseId }).catch(() => undefined);
    }
  }
}

function parseOptions(args: string[]): Record<string, string | boolean> {
  const options: Record<string, string | boolean> = {};

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];

    if (arg === "--dry-run") {
      options["dry-run"] = true;
      continue;
    }

    if (arg === "-c") {
      options.client = nextValue(args, (index += 1), arg);
      continue;
    }

    if (arg === "-s") {
      options.scope = nextValue(args, (index += 1), arg);
      continue;
    }

    if (arg.startsWith("--")) {
      const [name, inlineValue] = arg.slice(2).split("=", 2);
      options[name] = inlineValue ?? nextValue(args, (index += 1), arg);
      continue;
    }

    throw new Error(`Unexpected argument: ${arg}`);
  }

  return options;
}

function nextValue(args: string[], index: number, flag: string): string {
  const value = args[index];
  if (!value || value.startsWith("-")) {
    throw new Error(`Missing value for ${flag}`);
  }
  return value;
}

function stringOption(
  options: Record<string, string | boolean>,
  key: string,
): string | undefined {
  const value = options[key];
  return typeof value === "string" ? value : undefined;
}

function parseClient(value: string): ClientSelector {
  if (
    value === "codex" ||
    value === "claude-desktop" ||
    value === "cursor" ||
    value === "vscode" ||
    value === "all"
  ) {
    return value;
  }

  throw new Error(`Unsupported client: ${value}`);
}

function parseScope(value: string): ConfigScope {
  if (value === "user" || value === "project") {
    return value;
  }

  throw new Error(`Unsupported scope: ${value}`);
}

function printHelp() {
  console.log(`pgsandbox-mcp ${VERSION}

Usage:
  pgsandbox-mcp                      Start the MCP server over stdio
  pgsandbox-mcp stdio                Start the MCP server over stdio
  pgsandbox-mcp setup [options]      Write MCP client config
  pgsandbox-mcp doctor [options]     Check config and Postgres connectivity
  pgsandbox-mcp smoke-test [options] Create, query, and delete a sandbox

Setup options:
  --client <client>                  codex, cursor, vscode, claude-desktop, all
  --scope <scope>                    user or project
  --admin-url <url>                  Admin Postgres URL to write into config
  --command <command>                Command MCP clients should run
  --name <name>                      Server name in MCP config
  --dry-run                          Print config without writing
`);
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`pgsandbox-mcp failed to start: ${message}`);
  process.exit(1);
});
