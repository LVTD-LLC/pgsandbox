import { describe, expect, it } from "vitest";
import { makeSandboxNames, quoteIdent, quoteLiteral, slugifyNameHint } from "./names.js";

describe("names", () => {
  it("slugifies human hints", () => {
    expect(slugifyNameHint("Try Django 5 migrations!")).toBe("try_django_5_migrations");
    expect(slugifyNameHint("")).toBe("sandbox");
  });

  it("generates bounded database and role names", () => {
    const names = makeSandboxNames({
      prefix: "pgsandbox",
      nameHint: "a very long thing with spaces and punctuation that should be truncated",
    });

    expect(names.databaseName).toMatch(/^pgsandbox_a_very_long_thing/);
    expect(names.databaseName.length).toBeLessThanOrEqual(63);
    expect(names.roleName.length).toBeLessThanOrEqual(63);
  });

  it("keeps role names distinct when database names hit the identifier limit", () => {
    const names = makeSandboxNames({
      prefix: "p".repeat(80),
      nameHint: "x".repeat(80),
    });

    expect(names.databaseName.length).toBe(63);
    expect(names.roleName.length).toBeLessThanOrEqual(63);
    expect(names.roleName).not.toBe(names.databaseName);
    expect(names.roleName.endsWith("_role")).toBe(true);
  });

  it("quotes identifiers and literals", () => {
    expect(quoteIdent('weird"name')).toBe('"weird""name"');
    expect(quoteLiteral("can't")).toBe("'can''t'");
  });
});
