import { expect, test } from "@playwright/test";
import { asOwner } from "./people.ts";

const base = () => process.env.CHARET_E2E_URL ?? "";

test.use({ extraHTTPHeaders: asOwner() });

test("a component's state is picked by its props and shown live", async ({ page }) => {
  await page.goto(`${base()}/p/demo/components`);
  const button = page.getByTestId("component-Button");
  await expect(button).toBeVisible();
  const code = button.locator("pre");
  await expect(code).toHaveText('<Button variant="default" size="default">Save</Button>');
  await expect(button.getByTestId("prop-Button-variant-default")).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  await button.getByTestId("prop-Button-variant-outline").click();
  await button.getByTestId("prop-Button-size-lg").click();
  await expect(code).toHaveText('<Button variant="outline" size="lg">Save</Button>');
  await expect(button.getByTestId("prop-Button-variant-default")).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  // The preview is the real component on the preview origin, not a picture.
  const frame = button.frameLocator('iframe[title="button-variant-outline-size-lg"]');
  await expect(frame.getByRole("button", { name: "Save" })).toBeVisible({ timeout: 30_000 });

  // A state that sets no props stays reachable beside the picker, and so do the props again.
  await button.getByTestId("state-Button-Loading").click();
  await expect(code).toHaveCount(0);
  await expect(
    button.frameLocator('iframe[title="button-loading"]').getByRole("button", { name: "Saving…" }),
  ).toBeVisible({ timeout: 30_000 });
  await button.getByTestId("prop-Button-variant-outline").click();
  // From a state with no props nothing is kept, so the first story with that value shows.
  await expect(code).toHaveText('<Button variant="outline" size="sm">Save</Button>');

  await button.getByTestId("story-button-variant-outline-size-sm").click();
  await expect(page).toHaveURL(/\/p\/demo\/t\/story\/button-variant-outline-size-sm$/);
});
