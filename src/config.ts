import { readFileSync } from "node:fs";
import { z } from "zod";

const DEFAULT_DATABASE_PREFIX = "pgsandbox";
const DEFAULT_TTL_MINUTES = 240;
const DEFAULT_MAX_TTL_MINUTES = 1440;

const profileSchema = z.object({
  name: z.string().min(1),
  adminUrl: z.string().url(),
  databasePrefix: z.string().min(1).default(DEFAULT_DATABASE_PREFIX),
  defaultTtlMinutes: z.number().int().positive().default(DEFAULT_TTL_MINUTES),
  maxTtlMinutes: z.number().int().positive().default(DEFAULT_MAX_TTL_MINUTES),
});

const configSchema = z.object({
  defaultProfile: z.string().optional(),
  profiles: z.array(profileSchema).min(1),
});

export type SandboxProfile = z.infer<typeof profileSchema>;

export type SandboxConfig = {
  defaultProfile: string;
  profiles: SandboxProfile[];
};

export function loadConfig(env: NodeJS.ProcessEnv = process.env): SandboxConfig {
  if (env.PGSANDBOX_CONFIG) {
    return parseConfigFile(readFileSync(env.PGSANDBOX_CONFIG, "utf8"));
  }

  if (!env.PGSANDBOX_ADMIN_DATABASE_URL) {
    throw new Error(
      "Set PGSANDBOX_ADMIN_DATABASE_URL or PGSANDBOX_CONFIG before starting pgsandbox-mcp.",
    );
  }

  const parsed = configSchema.parse({
    defaultProfile: env.PGSANDBOX_DEFAULT_PROFILE ?? "default",
    profiles: [
      {
        name: env.PGSANDBOX_DEFAULT_PROFILE ?? "default",
        adminUrl: env.PGSANDBOX_ADMIN_DATABASE_URL,
        databasePrefix: env.PGSANDBOX_DATABASE_PREFIX,
        defaultTtlMinutes: env.PGSANDBOX_DEFAULT_TTL_MINUTES
          ? Number(env.PGSANDBOX_DEFAULT_TTL_MINUTES)
          : undefined,
        maxTtlMinutes: env.PGSANDBOX_MAX_TTL_MINUTES
          ? Number(env.PGSANDBOX_MAX_TTL_MINUTES)
          : undefined,
      },
    ],
  });

  return normalizeConfig(parsed);
}

export function parseConfigFile(rawJson: string): SandboxConfig {
  return normalizeConfig(configSchema.parse(JSON.parse(rawJson)));
}

export function findProfile(
  config: SandboxConfig,
  profileName: string | undefined,
): SandboxProfile {
  const name = profileName ?? config.defaultProfile;
  const profile = config.profiles.find((candidate) => candidate.name === name);

  if (!profile) {
    throw new Error(`Unknown Postgres profile: ${name}`);
  }

  return profile;
}

function normalizeConfig(parsed: z.infer<typeof configSchema>): SandboxConfig {
  const defaultProfile = parsed.defaultProfile ?? parsed.profiles[0].name;

  if (!parsed.profiles.some((profile) => profile.name === defaultProfile)) {
    throw new Error(`Default profile does not exist: ${defaultProfile}`);
  }

  for (const profile of parsed.profiles) {
    if (profile.defaultTtlMinutes > profile.maxTtlMinutes) {
      throw new Error(
        `defaultTtlMinutes cannot exceed maxTtlMinutes for profile ${profile.name}`,
      );
    }
  }

  return {
    defaultProfile,
    profiles: parsed.profiles,
  };
}
