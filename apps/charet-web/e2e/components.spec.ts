// The Components view: the components the screens use, found in their captures and cut out of them.

import { expect, test } from "@playwright/test";
import { asOwner } from "./people.ts";

const base = () => process.env.CHARET_E2E_URL ?? "";

test.describe.configure({ mode: "serial" });
// The owner, as the admin link or the CLI would sign them in.
test.use({ extraHTTPHeaders: asOwner() });

test("shows the components the screens use, cut out of their captures", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  const captured = await request.post(`${base()}/api/projects/demo/captures`, {
    data: { themes: ["light"] },
  });
  expect(captured.ok()).toBe(true);

  await page.goto(`${base()}/p/demo/components`);
  const button = page.getByTestId("component-Button");
  await expect(button).toBeVisible();
  await expect(button.getByText("src/components/ui/button.tsx")).toBeVisible();
  await expect(button.getByText(/^used \d+×$/)).toBeVisible();
  const first = button.getByTestId(/^use-Button-/).first();
  await expect(first.locator("img")).toBeVisible();

  // A use opens the screen it was cut from.
  await first.click();
  await expect(page).toHaveURL(/\/p\/demo\/t\/screen\//);
});
