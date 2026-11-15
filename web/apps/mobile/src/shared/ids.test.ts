import { describe, expect, it } from "vitest";

import { newBankTranId, tranDtime, ymd } from "./ids";
import { dateTime, won } from "./format";

describe("ids", () => {
  it("bank_tran_id is AN(20) with U at position 11", () => {
    const id = newBankTranId();
    expect(id).toMatch(/^[A-Z0-9]{10}U[A-Z0-9]{9}$/);
  });
  it("tran_dtime is N(14)", () => {
    expect(tranDtime(new Date(Date.UTC(2026, 0, 3, 0, 12, 44)))).toBe("20260103091244");
    expect(ymd(new Date(Date.UTC(2026, 10, 12)))).toBe("20261112");
  });
});

describe.each([
  ["UTC", 0],
  ["Asia/Seoul", -540],
  ["America/Los_Angeles", 480],
] as const)("KST dates in %s", (zone, offset) => {
  it.each([
    ["2026-11-14T14:59:59Z", "20261114235959", "20261114"],
    ["2026-11-14T15:00:00Z", "20261115000000", "20261115"],
    ["2026-11-14T17:38:00Z", "20261115023800", "20261115"],
    ["2026-12-31T15:00:00Z", "20270101000000", "20270101"],
  ])("formats %s in KST", (instant, timestamp, date) => {
    const originalZone = process.env.TZ;
    try {
      process.env.TZ = zone;
      expect(new Date(Date.UTC(2026, 10, 14, 15)).getTimezoneOffset()).toBe(offset);
      expect.soft(tranDtime(new Date(instant))).toBe(timestamp);
      expect.soft(ymd(new Date(instant))).toBe(date);
    } finally {
      if (originalZone === undefined) delete process.env.TZ;
      else process.env.TZ = originalZone;
    }
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
