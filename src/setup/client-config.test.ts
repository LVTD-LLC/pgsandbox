import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { buildLaunchConfig, resolveTargets, writeClientConfig } from "./client-config.js";

describe("client config writers", () => {
  it("writes Cursor-compatible mcpServers JSON", async () => {
    const dir = await mkdtemp(join(tmpdir(), "pgsandbox-mcp-"));

    try {
      const [target] = resolveTargets({ client: "cursor", scope: "project", cwd: dir });
      const launch = buildLaunchConfig({
        adminUrl: "postgres://postgres:secret@localhost:5432/postgres",
      });

      await writeClientConfig(target, launch);
      const parsed = JSON.parse(await readFile(target.path, "utf8"));

      expect(parsed.mcpServers.pgsandbox.command).toBe("pgsandbox-mcp");
      expect(parsed.mcpServers.pgsandbox.args).toEqual(["stdio"]);
      expect(parsed.mcpServers.pgsandbox.env.PGSANDBOX_ADMIN_DATABASE_URL).toContain("secret");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("writes VS Code-compatible servers JSON", async () => {
    const dir = await mkdtemp(join(tmpdir(), "pgsandbox-mcp-"));

    try {
      const [target] = resolveTargets({ client: "vscode", scope: "project", cwd: dir });
      const launch = buildLaunchConfig({});

      await writeClientConfig(target, launch);
      const parsed = JSON.parse(await readFile(target.path, "utf8"));

      expect(parsed.servers.pgsandbox.type).toBe("stdio");
      expect(parsed.servers.pgsandbox.command).toBe("pgsandbox-mcp");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("upserts a Codex TOML server block without deleting other config", async () => {
    const dir = await mkdtemp(join(tmpdir(), "pgsandbox-mcp-"));

    try {
      const [target] = resolveTargets({ client: "codex", scope: "project", cwd: dir });
      await mkdir(join(dir, ".codex"), { recursive: true });
      await writeFile(
        target.path,
        'model = "gpt-5"\n\n[mcp_servers.existing]\ncommand = "other"\n',
        "utf8",
      );

      await writeClientConfig(target, buildLaunchConfig({ command: "/opt/bin/pgsandbox-mcp" }));
      const content = await readFile(target.path, "utf8");

      expect(content).toContain('model = "gpt-5"');
      expect(content).toContain("[mcp_servers.existing]");
      expect(content).toContain("[mcp_servers.pgsandbox]");
      expect(content).toContain('command = "/opt/bin/pgsandbox-mcp"');
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
