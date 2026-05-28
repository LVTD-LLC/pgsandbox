import { describe, expect, it } from "vitest";
import { assertSafeReadonlySql } from "./postgres.js";

describe("readonly SQL guard", () => {
  it("allows ordinary read queries", () => {
    expect(() => assertSafeReadonlySql("select * from users")).not.toThrow();
  });

  it("rejects transaction-control bypass attempts", () => {
    expect(() => assertSafeReadonlySql("rollback; drop table users")).toThrow("readonly SQL");
    expect(() => assertSafeReadonlySql("set session characteristics as transaction read write")).toThrow(
      "readonly SQL",
    );
  });
});
