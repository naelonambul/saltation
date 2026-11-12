// Display formats for KFTC string values.

export function won(value: string | number | undefined): string {
  const n = typeof value === "number" ? value : Number(value ?? 0);
  return `${n.toLocaleString("ko-KR")}원`;
}

export function dateTime(date: string, time: string): string {
  return `${date.slice(0, 4)}.${date.slice(4, 6)}.${date.slice(6, 8)} ${time.slice(0, 2)}:${time.slice(2, 4)}`;
}
