import { describe, expect, it } from "vitest";
import { findProfile, loadConfig, parseConfigFile } from "./config.js";

describe("config", () => {
  it("loads a single profile from env", () => {
    const config = loadConfig({
      PGSANDBOX_ADMIN_DATABASE_URL: "postgres://postgres:postgres@localhost:5432/postgres",
      PGSANDBOX_DATABASE_PREFIX: "agent_db",
    });

    expect(config.defaultProfile).toBe("default");
    expect(config.profiles[0]).toMatchObject({
      name: "default",
      databasePrefix: "agent_db",
      defaultTtlMinutes: 240,
      maxTtlMinutes: 1440,
    });
  });

  it("loads named profiles from JSON", () => {
    const config = parseConfigFile(
      JSON.stringify({
        defaultProfile: "pg17",
        profiles: [
          {
            name: "pg17",
            adminUrl: "postgres://postgres:postgres@localhost:5432/postgres",
          },
          {
            name: "pg16",
            adminUrl: "postgres://postgres:postgres@localhost:5433/postgres",
          },
        ],
      }),
    );

    expect(findProfile(config, undefined).name).toBe("pg17");
    expect(findProfile(config, "pg16").name).toBe("pg16");
  });

  it("rejects missing connection configuration", () => {
    expect(() => loadConfig({})).toThrow("PGSANDBOX_ADMIN_DATABASE_URL");
  });
});
