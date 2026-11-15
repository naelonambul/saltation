// Plan step 10: a person signs in, sees balances and history, and transfers.
// Runs in the Playwright container against the Compose stack; credentials of a
// seeded customer and a receiving account come from the generator's roster.
import { expect, test } from "@playwright/test";

import { won } from "../src/shared/format";

const phone = process.env.E2E_PHONE ?? "";
const password = process.env.E2E_PASSWORD ?? "";
const toAccount = process.env.E2E_TO_ACCOUNT ?? "";

test("login, balance, history, transfer", async ({ page }) => {
  test.skip(!phone || !password || !toAccount, "E2E_PHONE, E2E_PASSWORD and E2E_TO_ACCOUNT are required");

  const amount = 1001 + Math.floor(Math.random() * 8999);
  console.log(`transfer amount ${won(amount)}`);

  let historyUrl: URL | undefined;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.searchParams.has("from_date")) historyUrl = url;
  });
  const koreaDate = () => new Date(Date.now() + 9 * 3600_000).toISOString().slice(0, 10).replaceAll("-", "");
  function assertHistoryWindow(before: string, after: string) {
    expect(historyUrl).toBeDefined();
    const from = historyUrl!.searchParams.get("from_date");
    const to = historyUrl!.searchParams.get("to_date");
    console.log(`history window ${from}..${to} (browser zone ${process.env.E2E_TIMEZONE || "default"}, Korea dates ${before}, ${after})`);
    expect([before, after]).toContain(to);
  }

  await page.goto("/");
  await page.getByLabel("전화번호").fill(phone);
  await page.getByLabel("비밀번호").fill(password);
  await page.getByRole("button", { name: "로그인" }).click();

  const card = page.getByTestId("account-card").first();
  await expect(card).toBeVisible();
  await expect(page.getByTestId("balance").first()).toContainText("원");
  const before = await page.getByTestId("balance").first().innerText();

  const firstHistoryBefore = koreaDate();
  await card.click();
  // The window is checked first so that a wrong date is printed even when the list is empty.
  await expect.poll(() => historyUrl?.searchParams.get("to_date")).toBeTruthy();
  const firstHistoryAfter = koreaDate();
  assertHistoryWindow(firstHistoryBefore, firstHistoryAfter);
  await expect(page.getByTestId("history")).toBeVisible();
  await page.getByRole("button", { name: "뒤로" }).click();

  await page.getByRole("button", { name: "이체하기" }).click();
  await page.getByLabel("받는 계좌번호 (살테이션은행)").fill(toAccount);
  await page.getByLabel("받는 분").fill("받는분");
  await page.getByLabel("금액(원)").fill(String(amount));
  await page.getByRole("button", { name: "다음" }).click();
  const transferBefore = koreaDate();
  await page.getByRole("button", { name: "보내기" }).click();
  await expect(page.getByText("을 보냈습니다")).toBeVisible();
  await page.getByRole("button", { name: "확인" }).click();

  await expect(page.getByTestId("balance").first()).not.toHaveText(before);
  historyUrl = undefined;
  const secondHistoryBefore = koreaDate();
  await page.getByTestId("account-card").first().click();
  await expect.poll(() => historyUrl?.searchParams.get("to_date")).toBeTruthy();
  const secondHistoryAfter = koreaDate();
  assertHistoryWindow(secondHistoryBefore, secondHistoryAfter);
  await expect(page.getByTestId("history")).toBeVisible();
  const transferDates = [transferBefore, secondHistoryAfter]
    .map((date) => date.replace(/^(\d{4})(\d{2})(\d{2})$/, "$1.$2.$3"));
  await expect(page.getByTestId("history").getByRole("listitem")
    .filter({ hasText: new RegExp(transferDates.map((date) => date.replaceAll(".", "\\.")).join("|")) })
    .filter({ hasText: `−${won(amount)}` }).first()).toBeVisible();
});
