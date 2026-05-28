import { randomBytes } from "node:crypto";
import { Client } from "pg";
import type { SandboxConfig, SandboxProfile } from "./config.js";
import { findProfile } from "./config.js";
import { makeSandboxNames, quoteIdent, quoteLiteral } from "./names.js";

const METADATA_TABLE = "pgsandbox_databases";
const DEFAULT_ROW_LIMIT = 100;

type SandboxRecord = {
  database_id: string;
  profile_name: string;
  database_name: string;
  role_name: string;
  role_password: string;
  owner: string | null;
  purpose: string | null;
  labels: Record<string, unknown>;
  created_at: string;
  expires_at: string;
  deleted_at: string | null;
};

export class PostgresSandboxManager {
  constructor(private readonly config: SandboxConfig) {}

  async createDatabase(input: {
    profile?: string;
    nameHint?: string;
    ttlMinutes?: number;
    owner?: string;
    labels?: Record<string, unknown>;
  }) {
    const profile = findProfile(this.config, input.profile);
    const ttlMinutes = clampTtl(input.ttlMinutes, profile);
    const { databaseId, databaseName, roleName } = makeSandboxNames({
      prefix: profile.databasePrefix,
      nameHint: input.nameHint,
    });
    const rolePassword = randomBytes(24).toString("base64url");
    const expiresAt = new Date(Date.now() + ttlMinutes * 60_000);

    await withAdminClient(profile, async (client) => {
      await ensureMetadataTable(client);
      let createdRole = false;
      let createdDatabase = false;

      try {
        await client.query(
          `CREATE ROLE ${quoteIdent(roleName)} LOGIN PASSWORD ${quoteLiteral(rolePassword)}`,
        );
        createdRole = true;
        await client.query(
          `CREATE DATABASE ${quoteIdent(databaseName)} OWNER ${quoteIdent(roleName)}`,
        );
        createdDatabase = true;
        await client.query(
          `
            INSERT INTO ${quoteIdent(METADATA_TABLE)}
              (database_id, profile_name, database_name, role_name, role_password, owner, purpose, labels, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9)
          `,
          [
            databaseId,
            profile.name,
            databaseName,
            roleName,
            rolePassword,
            input.owner ?? null,
            input.nameHint ?? null,
            JSON.stringify(input.labels ?? {}),
            expiresAt.toISOString(),
          ],
        );
      } catch (error) {
        if (createdDatabase) {
          await terminateDatabaseConnections(client, databaseName).catch(() => undefined);
          await client
            .query(`DROP DATABASE IF EXISTS ${quoteIdent(databaseName)}`)
            .catch(() => undefined);
        }

        if (createdRole) {
          await client.query(`DROP ROLE IF EXISTS ${quoteIdent(roleName)}`).catch(() => undefined);
        }

        throw error;
      }
    });

    return {
      databaseId,
      profile: profile.name,
      databaseName,
      roleName,
      expiresAt: expiresAt.toISOString(),
      connectionString: buildConnectionString(profile.adminUrl, {
        databaseName,
        roleName,
        rolePassword,
      }),
    };
  }

  async deleteDatabase(input: { profile?: string; databaseId?: string; databaseName?: string }) {
    const profile = findProfile(this.config, input.profile);

    return withAdminClient(profile, async (client) => {
      await ensureMetadataTable(client);
      const record = await findRecord(client, profile.name, input);

      if (!record) {
        throw new Error("Database was not found in PGSandbox metadata.");
      }

      await terminateDatabaseConnections(client, record.database_name);
      await client.query(`DROP DATABASE IF EXISTS ${quoteIdent(record.database_name)}`);
      await client.query(`DROP ROLE IF EXISTS ${quoteIdent(record.role_name)}`);
      await client.query(
        `UPDATE ${quoteIdent(METADATA_TABLE)} SET deleted_at = now() WHERE database_id = $1`,
        [record.database_id],
      );

      return {
        databaseId: record.database_id,
        databaseName: record.database_name,
        deleted: true,
      };
    });
  }

  async getConnectionString(input: {
    profile?: string;
    databaseId?: string;
    databaseName?: string;
  }) {
    const profile = findProfile(this.config, input.profile);

    return withAdminClient(profile, async (client) => {
      await ensureMetadataTable(client);
      const record = await findRecord(client, profile.name, input);

      if (!record) {
        throw new Error("Database was not found in PGSandbox metadata.");
      }

      return {
        databaseId: record.database_id,
        databaseName: record.database_name,
        expiresAt: record.expires_at,
        connectionString: buildConnectionString(profile.adminUrl, {
          databaseName: record.database_name,
          roleName: record.role_name,
          rolePassword: record.role_password,
        }),
      };
    });
  }

  async listDatabases(input: { profile?: string; owner?: string } = {}) {
    const profile = findProfile(this.config, input.profile);

    return withAdminClient(profile, async (client) => {
      await ensureMetadataTable(client);
      const result = await client.query(
        `
          SELECT database_id, profile_name, database_name, role_name, owner, purpose, labels,
                 created_at, expires_at, deleted_at
          FROM ${quoteIdent(METADATA_TABLE)}
          WHERE profile_name = $1
            AND deleted_at IS NULL
            AND ($2::text IS NULL OR owner = $2)
          ORDER BY created_at DESC
          LIMIT 100
        `,
        [profile.name, input.owner ?? null],
      );

      return result.rows;
    });
  }

  async runSql(input: {
    profile?: string;
    databaseId?: string;
    databaseName?: string;
    sql: string;
    readonly?: boolean;
    rowLimit?: number;
  }) {
    const connection = await this.getConnectionString(input);
    const client = new Client({ connectionString: connection.connectionString });
    const startedAt = Date.now();

    await client.connect();
    try {
      if (input.readonly) {
        await client.query("BEGIN READ ONLY");
      }

      // values: [] forces the extended query protocol, rejecting multi-statement strings.
      const result = await client.query({ text: input.sql, values: [] });

      if (input.readonly) {
        await client.query("ROLLBACK");
      }

      const rowLimit = input.rowLimit ?? DEFAULT_ROW_LIMIT;
      return {
        databaseId: connection.databaseId,
        databaseName: connection.databaseName,
        rowCount: result.rowCount,
        rows: result.rows.slice(0, rowLimit),
        truncated: result.rows.length > rowLimit,
        elapsedMs: Date.now() - startedAt,
      };
    } catch (error) {
      if (input.readonly) {
        await client.query("ROLLBACK").catch(() => undefined);
      }
      throw error;
    } finally {
      await client.end();
    }
  }

  async describeSchema(input: { profile?: string; databaseId?: string; databaseName?: string }) {
    const connection = await this.getConnectionString(input);
    const client = new Client({ connectionString: connection.connectionString });

    await client.connect();
    try {
      const [tables, columns, indexes, extensions] = await Promise.all([
        client.query(`
          SELECT table_schema, table_name
          FROM information_schema.tables
          WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
          ORDER BY table_schema, table_name
        `),
        client.query(`
          SELECT table_schema, table_name, column_name, data_type, is_nullable
          FROM information_schema.columns
          WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
          ORDER BY table_schema, table_name, ordinal_position
        `),
        client.query(`
          SELECT schemaname, tablename, indexname, indexdef
          FROM pg_indexes
          WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
          ORDER BY schemaname, tablename, indexname
        `),
        client.query("SELECT extname, extversion FROM pg_extension ORDER BY extname"),
      ]);

      return {
        databaseId: connection.databaseId,
        databaseName: connection.databaseName,
        tables: tables.rows,
        columns: columns.rows,
        indexes: indexes.rows,
        extensions: extensions.rows,
      };
    } finally {
      await client.end();
    }
  }

  async cleanupExpired(input: { profile?: string; dryRun?: boolean } = {}) {
    const profile = findProfile(this.config, input.profile);

    return withAdminClient(profile, async (client) => {
      await ensureMetadataTable(client);
      const expired = await client.query<SandboxRecord>(
        `
          SELECT *
          FROM ${quoteIdent(METADATA_TABLE)}
          WHERE profile_name = $1
            AND deleted_at IS NULL
            AND expires_at <= now()
          ORDER BY expires_at ASC
          LIMIT 50
        `,
        [profile.name],
      );

      if (input.dryRun) {
        return { dryRun: true, selected: expired.rows };
      }

      const deleted = [];
      const failures = [];

      for (const record of expired.rows) {
        try {
          await terminateDatabaseConnections(client, record.database_name);
          await client.query(`DROP DATABASE IF EXISTS ${quoteIdent(record.database_name)}`);
          await client.query(`DROP ROLE IF EXISTS ${quoteIdent(record.role_name)}`);
          await client.query(
            `UPDATE ${quoteIdent(METADATA_TABLE)} SET deleted_at = now() WHERE database_id = $1`,
            [record.database_id],
          );
          deleted.push(record.database_id);
        } catch (error) {
          failures.push({
            databaseId: record.database_id,
            message: error instanceof Error ? error.message : String(error),
          });
        }
      }

      return { dryRun: false, deleted, failures };
    });
  }
}

async function withAdminClient<T>(
  profile: SandboxProfile,
  callback: (client: Client) => Promise<T>,
): Promise<T> {
  const client = new Client({ connectionString: profile.adminUrl });

  await client.connect();
  try {
    return await callback(client);
  } finally {
    await client.end();
  }
}

async function ensureMetadataTable(client: Client) {
  await client.query(`
    CREATE TABLE IF NOT EXISTS ${quoteIdent(METADATA_TABLE)} (
      database_id text PRIMARY KEY,
      profile_name text NOT NULL,
      database_name text NOT NULL UNIQUE,
      role_name text NOT NULL UNIQUE,
      role_password text NOT NULL,
      owner text,
      purpose text,
      labels jsonb NOT NULL DEFAULT '{}'::jsonb,
      created_at timestamptz NOT NULL DEFAULT now(),
      expires_at timestamptz NOT NULL,
      deleted_at timestamptz
    )
  `);
}

async function findRecord(
  client: Client,
  profileName: string,
  input: { databaseId?: string; databaseName?: string },
): Promise<SandboxRecord | undefined> {
  if (!input.databaseId && !input.databaseName) {
    throw new Error("Provide databaseId or databaseName.");
  }

  const result = await client.query<SandboxRecord>(
    `
      SELECT *
      FROM ${quoteIdent(METADATA_TABLE)}
      WHERE deleted_at IS NULL
        AND profile_name = $3
        AND (($1::text IS NOT NULL AND database_id = $1)
          OR ($2::text IS NOT NULL AND database_name = $2))
      LIMIT 1
    `,
    [input.databaseId ?? null, input.databaseName ?? null, profileName],
  );

  return result.rows[0];
}

async function terminateDatabaseConnections(client: Client, databaseName: string) {
  await client.query(
    `
      SELECT pg_terminate_backend(pid)
      FROM pg_stat_activity
      WHERE datname = $1
        AND pid <> pg_backend_pid()
    `,
    [databaseName],
  );
}

function buildConnectionString(
  adminUrl: string,
  credentials: { databaseName: string; roleName: string; rolePassword: string },
): string {
  const url = new URL(adminUrl);
  url.username = credentials.roleName;
  url.password = credentials.rolePassword;
  url.pathname = `/${credentials.databaseName}`;
  return url.toString();
}

function clampTtl(ttlMinutes: number | undefined, profile: SandboxProfile): number {
  if (ttlMinutes === undefined) {
    return profile.defaultTtlMinutes;
  }

  if (ttlMinutes > profile.maxTtlMinutes) {
    throw new Error(
      `ttlMinutes exceeds maxTtlMinutes (${profile.maxTtlMinutes}) for profile ${profile.name}`,
    );
  }

  return ttlMinutes;
}
