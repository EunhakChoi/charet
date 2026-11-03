import { expect, test } from "@playwright/test";
import { addMember, enter } from "./people.ts";

// A local charet shared from its Settings page (`stack.ts`): the owner comes in with their admin link;
// on either address everyone else goes by token (or looks as a visitor).
const local = () => process.env.CHARET_E2E_SHARE_URL ?? "";
const remote = () => process.env.CHARET_E2E_SHARE_REMOTE ?? "";
const shareToken = () => process.env.CHARET_E2E_SHARE_TOKEN ?? "";

test("through the shared address, members bring their token and others are visitors", async ({
  browser,
}) => {
  const owner = await (await browser.newContext()).newPage();
  await enter(owner, shareToken(), local());
  await owner.goto(`${local()}/settings/access`);
  const sharing = owner.getByRole("region", { name: "Sharing" });
  await expect(sharing).toBeVisible();
  await sharing.getByRole("textbox", { name: "Address people open" }).fill(`${remote()}/`);
  await sharing.getByRole("button", { name: "Turn sharing on" }).click();
  await expect(sharing.getByRole("link", { name: remote() })).toBeVisible();

  // Through the remote door nobody is the owner: without a token, a visitor.
  const outsider = await (await browser.newContext()).newPage();
  await outsider.goto(`${remote()}/`);
  await expect(outsider.getByRole("heading", { name: "Projects" })).toBeVisible();
  await expect(outsider.getByRole("button", { name: "Enter token" })).toBeVisible();
  await expect(outsider.getByRole("link", { name: "Settings" })).toHaveCount(0);

  const jun = await addMember(owner, "Jun", "editor", local());
  await enter(outsider, jun, remote());
  await expect(outsider.getByText("Jun", { exact: true })).toBeVisible();

  // The address changes while shared, without turning sharing off.
  const address = sharing.getByRole("textbox", { name: "Address people open" });
  await expect(address).toHaveValue(remote());
  await address.fill(`${remote()}/changed`);
  await sharing.getByRole("button", { name: "Apply" }).click();
  await expect(sharing.getByRole("link", { name: `${remote()}/changed` })).toBeVisible();

  await sharing.getByRole("button", { name: "Turn sharing off" }).click();
  await expect(sharing.getByRole("button", { name: "Turn sharing on" })).toBeVisible();
  // The remote door keeps the same rules whatever sharing says.
  const after = await outsider.request.get(`${remote()}/api/auth/me`);
  expect(after.status()).toBe(200);
});

test("sharing through Tailscale shows the machine's tailnet name", async ({ page }) => {
  await enter(page, shareToken(), local());
  await page.goto(`${local()}/settings/access`);
  const sharing = page.getByRole("region", { name: "Sharing" });
  await sharing.getByRole("radio", { name: "Tailscale network" }).click();
  await expect(sharing.getByText("Address: https://e2e.tail1.ts.net")).toBeVisible();
  await sharing.getByRole("button", { name: "Turn sharing on" }).click();
  await expect(sharing.getByRole("link", { name: "https://e2e.tail1.ts.net" })).toBeVisible();
  await sharing.getByRole("button", { name: "Turn sharing off" }).click();
  await expect(sharing.getByRole("button", { name: "Turn sharing on" })).toBeVisible();
  // ngrok asks for the account's authtoken (not tried here: it needs a real account).
  await sharing.getByRole("radio", { name: "ngrok" }).click();
  await expect(sharing.getByLabel("ngrok authtoken")).toHaveAttribute("type", "password");
});

test("sharing on the local network opens the remote door on this machine's IP", async ({
  page,
}) => {
  await enter(page, shareToken(), local());
  await page.goto(`${local()}/settings/access`);
  const sharing = page.getByRole("region", { name: "Sharing" });
  await sharing.getByRole("radio", { name: "Local network" }).click();
  await sharing.getByRole("button", { name: "Turn sharing on" }).click();
  const link = sharing.getByRole("link", { name: /^http:\/\/\d+\.\d+\.\d+\.\d+:\d+$/ });
  await expect(link).toBeVisible();
  const url = (await link.textContent()) ?? "";
  // Over the LAN address: a visitor, not this machine's admin.
  const me = await page.request.get(`${url}/api/auth/me`);
  expect(((await me.json()) as { kind: string }).kind).toBe("visitor");
  await sharing.getByRole("button", { name: "Turn sharing off" }).click();
  await expect(sharing.getByRole("button", { name: "Turn sharing on" })).toBeVisible();
  // The LAN listener is gone (a fresh connection is refused).
  const after = await fetch(`${url}/api/health`).then(
    (r) => r.status,
    () => 0,
  );
  expect(after).toBe(0);
});
