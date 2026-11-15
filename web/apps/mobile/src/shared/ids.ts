// Open Banking identifiers the client generates.

const ALNUM = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

function randomChars(n: number, alphabet = ALNUM): string {
  const bytes = crypto.getRandomValues(new Uint8Array(n));
  return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
}

/** KFTC bank_tran_id, AN(20): 10-character client code, `U`, 9 characters. */
export function newBankTranId(clientCode = "M2026SALTW"): string {
  return `${clientCode.slice(0, 10).padEnd(10, "0")}U${randomChars(9)}`;
}

/** KFTC tran_dtime, N(14): YYYYMMDDHHMMSS in KST (UTC+9), whatever the device's time zone. */
export function tranDtime(now = new Date()): string {
  const kst = new Date(now.getTime() + 9 * 3600_000);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${kst.getUTCFullYear()}${p(kst.getUTCMonth() + 1)}${p(kst.getUTCDate())}${p(kst.getUTCHours())}${p(kst.getUTCMinutes())}${p(kst.getUTCSeconds())}`;
}

/** YYYYMMDD for Korea's date. */
export function ymd(d: Date): string {
  return tranDtime(d).slice(0, 8);
}
