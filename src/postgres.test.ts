import { describe, expect, it } from "vitest";
import { assertSafeReadonlySql, buildRunSqlResponse } from "./postgres.js";

describe("readonly SQL guard", () => {
  it("allows ordinary read queries", () => {
    expect(() => assertSafeReadonlySql("select * from users")).not.toThrow();
  });

  it("rejects transaction-control bypass attempts", () => {
    expect(() => assertSafeReadonlySql("rollback; drop table users")).toThrow("readonly SQL");
    expect(() => assertSafeReadonlySql("abort; drop table users")).toThrow("readonly SQL");
    expect(() => assertSafeReadonlySql("end; drop table users")).toThrow("readonly SQL");
    expect(() => assertSafeReadonlySql("set session characteristics as transaction read write")).toThrow(
      "readonly SQL",
    );
  });
});

describe("runSql response formatting", () => {
  it("preserves helper truncation even after rows have been sliced", () => {
    const response = buildRunSqlResponse(
      { databaseId: "sandbox-id", databaseName: "pgsandbox_test" },
      { rowCount: null, rows: [{ id: 1 }], truncated: true },
      12,
    );

    expect(response).toEqual({
      databaseId: "sandbox-id",
      databaseName: "pgsandbox_test",
      rowCount: null,
      rows: [{ id: 1 }],
      truncated: true,
      elapsedMs: 12,
    });
  });
});
