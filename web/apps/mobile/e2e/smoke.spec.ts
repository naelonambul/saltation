// Plan step 10: a person signs in, sees balances and history, and transfers.
// Runs in the Playwright container against the Compose stack; credentials of a
// seeded customer and a receiving account come from the generator's roster.
import { expect, test } from "@playwright/test";

const phone = process.env.E2E_PHONE ?? "";
const password = process.env.E2E_PASSWORD ?? "";
const toAccount = process.env.E2E_TO_ACCOUNT ?? "";

test("login, balance, history, transfer", async ({ page }) => {
  test.skip(!phone || !password || !toAccount, "E2E_PHONE, E2E_PASSWORD and E2E_TO_ACCOUNT are required");

  await page.goto("/");
  await page.getByLabel("전화번호").fill(phone);
  await page.getByLabel("비밀번호").fill(password);
  await page.getByRole("button", { name: "로그인" }).click();

  const card = page.getByTestId("account-card").first();
  await expect(card).toBeVisible();
  await expect(page.getByTestId("balance").first()).toContainText("원");
  const before = await page.getByTestId("balance").first().innerText();

  await card.click();
  await expect(page.getByTestId("history")).toBeVisible();
  await page.getByRole("button", { name: "뒤로" }).click();

  await page.getByRole("button", { name: "이체하기" }).click();
  await page.getByLabel("받는 계좌번호 (살테이션은행)").fill(toAccount);
  await page.getByLabel("받는 분").fill("받는분");
  await page.getByLabel("금액(원)").fill("1000");
  await page.getByRole("button", { name: "다음" }).click();
  await page.getByRole("button", { name: "보내기" }).click();
  await expect(page.getByText("을 보냈습니다")).toBeVisible();
  await page.getByRole("button", { name: "확인" }).click();

  await expect(page.getByTestId("balance").first()).not.toHaveText(before);
});
