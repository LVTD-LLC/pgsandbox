import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import type { SandboxConfig } from "./config.js";
import { PostgresSandboxManager } from "./postgres.js";

const profileSchema = z.string().optional();
const databaseSelectorSchema = {
  profile: profileSchema,
  databaseId: z.string().optional(),
  databaseName: z.string().optional(),
};

export function createServer(config: SandboxConfig): McpServer {
  const manager = new PostgresSandboxManager(config);
  const server = new McpServer({
    name: "pgsandbox-mcp",
    version: "0.1.0",
  });

  server.registerTool(
    "create_database",
    {
      title: "Create database",
      description: "Create an isolated Postgres sandbox database and login role.",
      inputSchema: {
        profile: profileSchema,
        nameHint: z.string().optional(),
        ttlMinutes: z.number().int().positive().optional(),
        owner: z.string().optional(),
        labels: z.record(z.string(), z.unknown()).optional(),
      },
    },
    async (input) => jsonResult(await manager.createDatabase(input)),
  );

  server.registerTool(
    "delete_database",
    {
      title: "Delete database",
      description: "Delete a sandbox database and role created by PGSandbox.",
      inputSchema: databaseSelectorSchema,
    },
    async (input) => jsonResult(await manager.deleteDatabase(input)),
  );

  server.registerTool(
    "get_connection_string",
    {
      title: "Get connection string",
      description: "Return the connection string for a sandbox database.",
      inputSchema: databaseSelectorSchema,
    },
    async (input) => jsonResult(await manager.getConnectionString(input)),
  );

  server.registerTool(
    "run_sql",
    {
      title: "Run SQL",
      description: "Run SQL against a sandbox database.",
      inputSchema: {
        ...databaseSelectorSchema,
        sql: z.string().min(1),
        readonly: z.boolean().optional(),
        rowLimit: z.number().int().positive().max(1000).optional(),
      },
    },
    async (input) => jsonResult(await manager.runSql(input)),
  );

  server.registerTool(
    "describe_schema",
    {
      title: "Describe schema",
      description: "Return schema metadata for a sandbox database.",
      inputSchema: databaseSelectorSchema,
    },
    async (input) => jsonResult(await manager.describeSchema(input)),
  );

  server.registerTool(
    "list_databases",
    {
      title: "List databases",
      description: "List sandbox databases known to PGSandbox.",
      inputSchema: {
        profile: profileSchema,
        owner: z.string().optional(),
      },
    },
    async (input) => jsonResult(await manager.listDatabases(input)),
  );

  server.registerTool(
    "cleanup_expired",
    {
      title: "Cleanup expired",
      description: "Delete expired sandbox databases.",
      inputSchema: {
        profile: profileSchema,
        dryRun: z.boolean().optional(),
      },
    },
    async (input) => jsonResult(await manager.cleanupExpired(input)),
  );

  return server;
}

function jsonResult(value: unknown) {
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(value, null, 2),
      },
    ],
  };
}
