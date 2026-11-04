// Web agent accounts: a sign-in under way keeps its code when its page is opened again, there is
// only one at a time per provider, and cancelling it leaves the provider signed out. Claude's
// sign-in starts without any call to the provider (its page shows the code), so this runs against
// the real stack without a network.

import { expect, test } from "@playwright/test";
import { asOwner } from "./people.ts";

const base = () => process.env.CHARET_E2E_URL ?? "";

test.use({ extraHTTPHeaders: asOwner() });

test("opening a waiting sign-in again shows the same page; cancelling ends it", async ({
  page,
  context,
}) => {
  // The provider's page is not fetched; only the addresses opened matter.
  const visits: string[] = [];
  await context.route("https://claude.ai/**", (route) => {
    visits.push(route.request().url());
    return route.fulfill({ contentType: "text/html", body: "<p>sign-in</p>" });
  });
  await page.goto(`${base()}/settings/agent`);
  const claude = page.getByRole("region", { name: "Anthropic" });
  await expect(claude.getByText("Sign in", { exact: true })).toBeVisible();

  const first = page.waitForEvent("popup");
  await claude.getByRole("button", { name: "Sign in with Anthropic" }).click();
  await first;
  await expect(claude.getByRole("button", { name: "Finish sign-in" })).toBeVisible();
  await expect.poll(() => visits.length).toBe(1);
  const state = new URL(visits[0] ?? "").searchParams.get("state");
  expect(state).toBeTruthy();

  // Open the page again: the same address (the sign-in under way, not a new one), so a code
  // already copied stays good. The browser reuses the one sign-in window.
  await claude.getByRole("button", { name: "Open the page again" }).click();
  await expect.poll(() => visits.length).toBe(2);
  expect(new URL(visits[1] ?? "").searchParams.get("state")).toBe(state);
  const status = await page.request.get(`${base()}/api/admin/agent/providers/anthropic/oauth`);
  expect(new URL(((await status.json()) as { url: string }).url).searchParams.get("state")).toBe(
    state,
  );

  // A code that belongs to another sign-in is refused and the sign-in stays.
  await claude.getByRole("textbox", { name: "Sign-in code for Anthropic" }).fill("abc#another");
  await claude.getByRole("button", { name: "Finish sign-in" }).click();
  await expect(claude.getByRole("alert")).toContainText("another sign-in");
  await expect(claude.getByRole("button", { name: "Finish sign-in" })).toBeVisible();

  // One sign-in at a time: a second start is refused while this one waits.
  const second = await page.request.post(`${base()}/api/admin/agent/providers/anthropic/oauth`);
  expect(second.status()).toBe(409);

  // Cancelling ends only the sign-in; nobody was signed in, so nobody is now.
  await claude.getByRole("button", { name: "Cancel" }).click();
  await expect(claude.getByRole("button", { name: "Sign in with Anthropic" })).toBeVisible();
  const after = await page.request.get(`${base()}/api/admin/agent/providers/anthropic/oauth`);
  expect(((await after.json()) as { state: string }).state).toBe("signed_out");
});
