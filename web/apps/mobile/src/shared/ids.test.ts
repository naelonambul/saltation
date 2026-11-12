import { describe, expect, it } from "vitest";

import { newBankTranId, tranDtime, ymd } from "./ids";
import { dateTime, won } from "./format";

describe("ids", () => {
  it("bank_tran_id is AN(20) with U at position 11", () => {
    const id = newBankTranId();
    expect(id).toMatch(/^[A-Z0-9]{10}U[A-Z0-9]{9}$/);
  });
  it("tran_dtime is N(14)", () => {
    expect(tranDtime(new Date(2026, 0, 3, 9, 12, 44))).toBe("20260103091244");
    expect(ymd(new Date(2026, 10, 12))).toBe("20261112");
  });
});

describe("format", () => {
  it("formats won and KFTC date-time", () => {
    expect(won("1234567")).toBe("1,234,567원");
    expect(dateTime("20260103", "091244")).toBe("2026.01.03 09:12");
  });
});

describe("session", () => {
  it("random IDs do not need a secure context", async () => {
    const { randomHex } = await import("./session");
    expect(randomHex(16)).toMatch(/^[0-9a-f]{32}$/);
  });
});
