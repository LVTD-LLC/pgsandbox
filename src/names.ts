import { randomUUID } from "node:crypto";

const MAX_IDENTIFIER_LENGTH = 63;

export function slugifyNameHint(value: string | undefined): string {
  const slug = (value ?? "sandbox")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .replace(/_+/g, "_");

  return slug || "sandbox";
}

export function makeSandboxNames(input: {
  prefix: string;
  nameHint?: string;
}): { databaseId: string; databaseName: string; roleName: string } {
  const databaseId = randomUUID();
  const shortId = databaseId.replaceAll("-", "").slice(0, 10);
  const prefix = slugifyNameHint(input.prefix);
  const hint = slugifyNameHint(input.nameHint).slice(0, 28);
  const base = trimIdentifier(`${prefix}_${hint}_${shortId}`);

  return {
    databaseId,
    databaseName: base,
    roleName: `${base.slice(0, MAX_IDENTIFIER_LENGTH - 5).replace(/_+$/g, "")}_role`,
  };
}

export function quoteIdent(identifier: string): string {
  if (!identifier || identifier.length > MAX_IDENTIFIER_LENGTH) {
    throw new Error(`Invalid Postgres identifier length: ${identifier}`);
  }

  return `"${identifier.replaceAll('"', '""')}"`;
}

export function quoteLiteral(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

function trimIdentifier(value: string): string {
  return value.slice(0, MAX_IDENTIFIER_LENGTH).replace(/_+$/g, "");
}
