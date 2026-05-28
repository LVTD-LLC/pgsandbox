import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { homedir, platform } from "node:os";
import { dirname, join } from "node:path";

const ADMIN_DATABASE_URL_ENV = "PGSANDBOX_ADMIN_DATABASE_URL";

export type SupportedClient = "codex" | "claude-desktop" | "cursor" | "vscode";
export type ClientSelector = SupportedClient | "all";
export type ConfigScope = "user" | "project";
export type ConfigFormat = "codex-toml" | "mcp-json" | "vscode-json";

export interface McpLaunchConfig {
  name: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
}

export interface ConfigTarget {
  client: SupportedClient;
  scope: ConfigScope;
  path: string;
  format: ConfigFormat;
}

export interface WriteResult {
  target: ConfigTarget;
  action: "updated" | "would_update";
  content: string;
}

const SUPPORTED_CLIENTS: SupportedClient[] = ["codex", "claude-desktop", "cursor", "vscode"];

export function buildLaunchConfig(options: {
  name?: string;
  command?: string;
  adminUrl?: string;
}): McpLaunchConfig {
  return {
    name: options.name ?? "pgsandbox",
    command: options.command ?? "pgsandbox-mcp",
    args: ["stdio"],
    env: options.adminUrl ? { [ADMIN_DATABASE_URL_ENV]: options.adminUrl } : undefined
  };
}

export function resolveTargets(options: {
  client: ClientSelector;
  scope: ConfigScope;
  cwd: string;
}): ConfigTarget[] {
  const clients = options.client === "all" ? SUPPORTED_CLIENTS : [options.client];
  return clients.map((client) => targetForClient(client, options.scope, options.cwd));
}

export async function writeClientConfig(
  target: ConfigTarget,
  launch: McpLaunchConfig,
  dryRun = false
): Promise<WriteResult> {
  const content = await nextConfigContent(target, launch);
  if (!dryRun) {
    await mkdir(dirname(target.path), { recursive: true });
    await writeFile(target.path, content, "utf8");
  }

  return {
    target,
    action: dryRun ? "would_update" : "updated",
    content
  };
}

export function configSnippet(target: ConfigTarget, launch: McpLaunchConfig): string {
  if (target.format === "codex-toml") {
    return codexTomlBlock(launch);
  }

  const rootKey = target.format === "vscode-json" ? "servers" : "mcpServers";
  const entry = jsonEntryForTarget(target, launch);
  return JSON.stringify({ [rootKey]: { [launch.name]: entry } }, null, 2);
}

export function detectExistingClientConfigs(cwd = process.cwd()): ConfigTarget[] {
  return SUPPORTED_CLIENTS.flatMap((client) => {
    const scopes: ConfigScope[] = client === "claude-desktop" ? ["user"] : ["user", "project"];
    return scopes
      .map((scope) => targetForClient(client, scope, cwd))
      .filter((target) => existsSync(target.path));
  });
}

async function nextConfigContent(target: ConfigTarget, launch: McpLaunchConfig): Promise<string> {
  if (target.format === "codex-toml") {
    const existing = await readOptional(target.path);
    return upsertCodexToml(existing, launch);
  }

  const existing = await readJsonObject(target.path);
  const rootKey = target.format === "vscode-json" ? "servers" : "mcpServers";
  const root = asRecord(existing[rootKey]);
  root[launch.name] = jsonEntryForTarget(target, launch);
  existing[rootKey] = root;
  return `${JSON.stringify(existing, null, 2)}\n`;
}

function targetForClient(client: SupportedClient, scope: ConfigScope, cwd: string): ConfigTarget {
  if (client === "claude-desktop" && scope === "project") {
    throw new Error("Claude Desktop only supports user-scoped MCP configuration.");
  }

  const home = homedir();
  const os = platform();

  if (scope === "project") {
    if (client === "codex") {
      return { client, scope, path: join(cwd, ".codex", "config.toml"), format: "codex-toml" };
    }
    if (client === "cursor") {
      return { client, scope, path: join(cwd, ".cursor", "mcp.json"), format: "mcp-json" };
    }
    if (client === "vscode") {
      return { client, scope, path: join(cwd, ".vscode", "mcp.json"), format: "vscode-json" };
    }
  }

  if (client === "codex") {
    return { client, scope, path: join(home, ".codex", "config.toml"), format: "codex-toml" };
  }
  if (client === "cursor") {
    return { client, scope, path: join(home, ".cursor", "mcp.json"), format: "mcp-json" };
  }
  if (client === "claude-desktop") {
    return {
      client,
      scope,
      path: claudeDesktopConfigPath(home, os),
      format: "mcp-json"
    };
  }

  return {
    client,
    scope,
    path: vscodeUserConfigPath(home, os),
    format: "vscode-json"
  };
}

function claudeDesktopConfigPath(home: string, os: NodeJS.Platform): string {
  if (os === "darwin") {
    return join(home, "Library", "Application Support", "Claude", "claude_desktop_config.json");
  }
  if (os === "win32") {
    return join(process.env.APPDATA ?? join(home, "AppData", "Roaming"), "Claude", "claude_desktop_config.json");
  }
  return join(home, ".config", "Claude", "claude_desktop_config.json");
}

function vscodeUserConfigPath(home: string, os: NodeJS.Platform): string {
  if (os === "darwin") {
    return join(home, "Library", "Application Support", "Code", "User", "mcp.json");
  }
  if (os === "win32") {
    return join(process.env.APPDATA ?? join(home, "AppData", "Roaming"), "Code", "User", "mcp.json");
  }
  return join(home, ".config", "Code", "User", "mcp.json");
}

function jsonEntryForTarget(target: ConfigTarget, launch: McpLaunchConfig): Record<string, unknown> {
  const entry: Record<string, unknown> = {
    command: launch.command,
    args: launch.args
  };

  if (target.format === "vscode-json") {
    entry.type = "stdio";
  }

  if (launch.env && Object.keys(launch.env).length > 0) {
    entry.env = launch.env;
  }

  return entry;
}

function upsertCodexToml(existing: string, launch: McpLaunchConfig): string {
  const block = codexTomlBlock(launch).trimEnd();
  const lines = existing ? existing.split(/\r?\n/) : [];
  const start = lines.findIndex((line) => isCodexServerHeader(line, launch.name));

  if (start === -1) {
    const prefix = existing.trim() ? `${existing.replace(/\s*$/, "\n\n")}` : "";
    return `${prefix}${block}\n`;
  }

  let end = start + 1;
  while (end < lines.length && !/^\s*\[/.test(lines[end])) {
    end += 1;
  }

  lines.splice(start, end - start, ...block.split("\n"));
  return `${lines.join("\n").replace(/\s*$/, "")}\n`;
}

function codexTomlBlock(launch: McpLaunchConfig): string {
  const lines = [
    `[mcp_servers.${tomlKey(launch.name)}]`,
    `command = ${tomlString(launch.command)}`,
    `args = [${launch.args.map(tomlString).join(", ")}]`
  ];

  if (launch.env && Object.keys(launch.env).length > 0) {
    const entries = Object.entries(launch.env)
      .map(([key, value]) => `${tomlKey(key)} = ${tomlString(value)}`)
      .join(", ");
    lines.push(`env = { ${entries} }`);
  }

  return `${lines.join("\n")}\n`;
}

async function readOptional(path: string): Promise<string> {
  try {
    return await readFile(path, "utf8");
  } catch (error) {
    if (isNodeError(error) && error.code === "ENOENT") {
      return "";
    }
    throw error;
  }
}

async function readJsonObject(path: string): Promise<Record<string, unknown>> {
  const content = await readOptional(path);
  if (!content.trim()) {
    return {};
  }

  const parsed = JSON.parse(content) as unknown;
  return asRecord(parsed);
}

function asRecord(value: unknown): Record<string, unknown> {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return {};
}

function isCodexServerHeader(line: string, name: string): boolean {
  const trimmed = line.trim();
  return trimmed === `[mcp_servers.${name}]` || trimmed === `[mcp_servers.${tomlKey(name)}]`;
}

function tomlKey(value: string): string {
  return /^[A-Za-z0-9_-]+$/.test(value) ? value : tomlString(value);
}

function tomlString(value: string): string {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error;
}
