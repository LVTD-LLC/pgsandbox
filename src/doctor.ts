import { access } from "node:fs/promises";
import { constants } from "node:fs";
import { Client } from "pg";
import { loadConfig } from "./config.js";
import { detectExistingClientConfigs } from "./setup/client-config.js";

export interface DoctorOptions {
  adminUrl?: string;
  cwd?: string;
}

export async function runDoctor(options: DoctorOptions = {}): Promise<{
  ok: boolean;
  lines: string[];
}> {
  const lines: string[] = [];
  let ok = true;

  lines.push(`CLI: ${process.argv[1]}`);

  let config;
  try {
    config = loadConfig(
      options.adminUrl
        ? { ...process.env, PGSANDBOX_ADMIN_DATABASE_URL: options.adminUrl }
        : process.env,
    );
  } catch (error) {
    ok = false;
    lines.push(error instanceof Error ? error.message : String(error));
  }

  if (config) {
    for (const profile of config.profiles) {
      lines.push(`Profile ${profile.name}: ${maskConnectionString(profile.adminUrl)}`);
      const postgresOk = await checkPostgres(profile.adminUrl);
      ok = ok && postgresOk.ok;
      lines.push(`Postgres connection (${profile.name}): ${postgresOk.message}`);
    }
  }

  const configs = detectExistingClientConfigs(options.cwd);
  if (configs.length === 0) {
    lines.push("MCP client configs: none found yet");
  } else {
    for (const config of configs) {
      const readable = await canRead(config.path);
      lines.push(
        `MCP client config: ${config.client} ${config.scope} ${readable ? "found" : "unreadable"} at ${config.path}`
      );
    }
  }

  return { ok, lines };
}

async function checkPostgres(adminUrl: string): Promise<{ ok: boolean; message: string }> {
  const client = new Client({
    connectionString: adminUrl,
    connectionTimeoutMillis: 3000,
  });

  try {
    await client.connect();
    await client.query("SELECT 1");
    return { ok: true, message: "ok" };
  } catch (error) {
    return {
      ok: false,
      message: error instanceof Error ? error.message : String(error)
    };
  } finally {
    await client.end().catch(() => undefined);
  }
}

export function maskConnectionString(value: string): string {
  try {
    const url = new URL(value);
    if (url.password) {
      url.password = "****";
    }
    return url.toString();
  } catch {
    return value.replace(/(:\/\/[^:\s]+:)([^@\s]+)(@)/, "$1****$3");
  }
}

async function canRead(path: string): Promise<boolean> {
  try {
    await access(path, constants.R_OK);
    return true;
  } catch {
    return false;
  }
}
