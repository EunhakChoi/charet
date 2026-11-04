// Web agent accounts: a sign-in under way keeps its code when its page is opened again, there is
// only one at a time per provider, and cancelling it leaves the provider signed out. Starting a
// sign-in makes no call to the provider (the person pastes back what its page ends with), so this
// runs against the real stack without a network.

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

test("ChatGPT signs in with the address its page ends on", async ({ page, context }) => {
  const visits: string[] = [];
  await context.route("https://auth.openai.com/**", (route) => {
    visits.push(route.request().url());
    return route.fulfill({ contentType: "text/html", body: "<p>sign-in</p>" });
  });
  await page.goto(`${base()}/settings/agent`);
  const chatgpt = page.getByRole("region", { name: "OpenAI" });
  const popup = page.waitForEvent("popup");
  await chatgpt.getByRole("button", { name: "Sign in with OpenAI" }).click();
  await popup;
  await expect(chatgpt.getByRole("button", { name: "Finish sign-in" })).toBeVisible();
  await expect.poll(() => visits.length).toBe(1);
  const page1 = new URL(visits[0] ?? "");
  expect(page1.pathname).toBe("/oauth/authorize");
  expect(page1.searchParams.get("redirect_uri")).toBe("http://localhost:1455/auth/callback");
  expect(page1.searchParams.get("code_challenge_method")).toBe("S256");

  // The address of another sign-in is refused and the sign-in stays.
  const box = chatgpt.getByRole("textbox", { name: "Sign-in code for OpenAI" });
  await box.fill("http://localhost:1455/auth/callback?code=abc&state=another");
  await chatgpt.getByRole("button", { name: "Finish sign-in" }).click();
  await expect(chatgpt.getByRole("alert")).toContainText("another sign-in");
  await expect(chatgpt.getByRole("button", { name: "Finish sign-in" })).toBeVisible();

  await chatgpt.getByRole("button", { name: "Cancel" }).click();
  await expect(chatgpt.getByRole("button", { name: "Sign in with OpenAI" })).toBeVisible();
});
