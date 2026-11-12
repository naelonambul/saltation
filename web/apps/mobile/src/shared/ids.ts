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

/** KFTC tran_dtime, N(14): YYYYMMDDHHMMSS in local time. */
export function tranDtime(now = new Date()): string {
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${now.getFullYear()}${p(now.getMonth() + 1)}${p(now.getDate())}${p(now.getHours())}${p(now.getMinutes())}${p(now.getSeconds())}`;
}

/** YYYYMMDD for a date. */
export function ymd(d: Date): string {
  return tranDtime(d).slice(0, 8);
}
