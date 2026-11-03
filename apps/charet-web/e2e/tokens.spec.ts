import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { expect, test } from "@playwright/test";
import { addMember, adminToken, base, enter } from "./people.ts";

// The public server (`stack.ts`): members come in by token, everyone else is a visitor.

test.describe.configure({ mode: "serial" });

test("a visitor looks around under a made-up name, then enters a token", async ({ page }) => {
  await page.goto(`${base()}/`);
  await expect(page.getByRole("heading", { name: "Projects" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Settings" })).toHaveCount(0);
  // An adjective and an animal, the same on every page load.
  const name = page.getByText(/^[A-Z][a-z]+ [A-Z][a-z]+$/).first();
  const before = await name.textContent();
  await page.reload();
  await expect(page.getByText(before ?? "", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Enter token" }).click();
  await page.getByRole("textbox", { name: "Token" }).fill("not-a-token");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("alert")).toContainText("does not work");
  await page.getByRole("textbox", { name: "Token" }).fill(adminToken());
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("link", { name: "Settings" })).toBeVisible();
  await expect(page.getByText("Admin", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("button", { name: "Enter token" })).toBeVisible();
});

test("an admin adds a member whose link signs them in; a second browser signs the first out", async ({
  browser,
}) => {
  const admin = await (await browser.newContext()).newPage();
  await enter(admin, adminToken());
  await admin.getByRole("link", { name: "Settings" }).click();
  const members = admin.getByRole("region", { name: "Members" });
  await members.getByRole("textbox", { name: "Name", exact: true }).fill("Kim");
  await members.getByRole("radio", { name: "Commenter" }).first().click();
  await members.getByRole("button", { name: "Add member" }).click();
  const token = await members.getByRole("textbox", { name: "Token of Kim" }).inputValue();
  expect(token).toHaveLength(32);

  const laptop = await (await browser.newContext()).newPage();
  await enter(laptop, token);
  await expect(laptop.getByText("Kim", { exact: true })).toBeVisible();
  await expect(laptop.getByRole("link", { name: "Settings" })).toHaveCount(0);

  const phone = await (await browser.newContext()).newPage();
  await enter(phone, token);
  await laptop.reload();
  await expect(laptop.getByText(/used on another browser/)).toBeVisible();

  // A new token stops the old one and signs the phone out too.
  await admin.reload();
  await members.getByRole("button", { name: "New token" }).last().click();
  await expect(members.getByRole("textbox", { name: "Token of Kim" })).not.toHaveValue(token);
  await phone.reload();
  await expect(phone.getByRole("button", { name: "Enter token" })).toBeVisible();
});

test("with visitors turned off the server asks for a token", async ({ browser }) => {
  const admin = await (await browser.newContext()).newPage();
  await enter(admin, adminToken());
  await admin.goto(`${base()}/settings/access`);
  const visitors = admin.getByRole("region", { name: "Visitors" });
  await visitors.getByRole("radio", { name: "Token required" }).click();
  await expect(visitors.getByRole("radio", { name: "Token required" })).toBeChecked();
  try {
    const outsider = await (await browser.newContext()).newPage();
    await outsider.goto(`${base()}/`);
    await expect(outsider.getByText("This server needs a token")).toBeVisible();
    const viewer = await addMember(admin, "Lee", "viewer");
    await outsider.getByRole("textbox", { name: "Token" }).fill(viewer);
    await outsider.getByRole("button", { name: "Sign in" }).click();
    await expect(outsider.getByRole("heading", { name: "Projects" })).toBeVisible();
  } finally {
    await visitors.getByRole("radio", { name: "Can look" }).click();
    await expect(visitors.getByRole("radio", { name: "Can look" })).toBeChecked();
  }
});

test("a project can let visitors comment, even when the server asks for a token", async ({
  browser,
}) => {
  const admin = await (await browser.newContext()).newPage();
  await enter(admin, adminToken());
  await admin.goto(`${base()}/p/demo/screens`);
  await admin.getByRole("button", { name: "Access" }).click();
  const visitors = admin.getByRole("region", { name: "Visitors (no token)" });
  await visitors.getByRole("radio", { name: "Can comment" }).click();
  await expect(visitors.getByRole("radio", { name: "Can comment" })).toBeChecked();
  await admin.goto(`${base()}/settings/access`);
  const team = admin.getByRole("region", { name: "Visitors", exact: true });
  await team.getByRole("radio", { name: "Token required" }).click();
  await expect(team.getByRole("radio", { name: "Token required" })).toBeChecked();
  try {
    const outsider = await (await browser.newContext()).newPage();
    await outsider.goto(`${base()}/p/demo/t/screen/tasks-list`);
    // The page offers commenting (no "view only"), and the server takes the visitor's comment.
    await expect(outsider.getByText(/Click an element or a spot on the screen/)).toBeVisible();
    await expect(outsider.getByText("You can view this project but not comment.")).toHaveCount(0);
    const added = await outsider.request.post(`${base()}/api/projects/demo/comments`, {
      headers: { "x-charet-client": "e2e" },
      data: {
        kind: "screen",
        target: "tasks-list",
        at: { x: 30, y: 30 },
        body: "A visitor's note",
      },
    });
    expect(added.status()).toBe(201);
    await expect(outsider.getByText("A visitor's note")).toBeVisible();
    // The whole project stays with members.
    const checkout = await outsider.request.get(`${base()}/api/projects/demo/checkout`);
    expect(checkout.status()).toBe(403);
  } finally {
    await team.getByRole("radio", { name: "Can look" }).click();
    await expect(team.getByRole("radio", { name: "Can look" })).toBeChecked();
    await admin.request.put(`${base()}/api/projects/demo/visitors`, {
      headers: { "x-charet-client": "e2e" },
      data: { visitors: null },
    });
  }
});

test("charet login --token makes the CLI act as that member", async () => {
  const root = resolve(import.meta.dirname, "../../..");
  const config = mkdtempSync(join(tmpdir(), "charet-cli-login-"));
  const env = { ...process.env, CHARET_CONFIG_DIR: config, CHARET_URL: base() };
  const cli = join(root, "apps/charet-cli/src/main.ts");
  try {
    execFileSync(process.execPath, [cli, "login", "--token", adminToken()], { env });
    const whoami = execFileSync(process.execPath, [cli, "whoami"], { env }).toString();
    expect(whoami).toContain("Admin, admin");
  } finally {
    rmSync(config, { recursive: true, force: true });
  }
});

test("a private project shows only to the members the admin picks", async ({ browser }) => {
  const admin = await (await browser.newContext()).newPage();
  await enter(admin, adminToken());
  const created = await admin.request.post(`${base()}/api/projects`, {
    headers: { "x-charet-client": "e2e" },
    data: { key: "shared", title: "Shared" },
  });
  expect(created.status()).toBe(201);
  const eve = await (await browser.newContext()).newPage();
  await enter(eve, await addMember(admin, "Eve", "editor"));

  await admin.goto(`${base()}/p/shared/screens`);
  await admin.getByRole("button", { name: "Access" }).click();
  const access = admin.getByRole("region", { name: "Who can see it" });
  await access.getByRole("radio", { name: "Private" }).click();
  await expect(access.getByText(/Only the people picked below/)).toBeVisible();
  await eve.reload();
  await expect(eve.getByRole("link", { name: /Shared/ })).toHaveCount(0);

  // The checkbox follows the saved list, so it turns on a moment after the click.
  const member = access.getByRole("checkbox", { name: /Eve/ });
  await member.click();
  await expect(member).toBeChecked();
  await eve.reload();
  await expect(eve.getByRole("link", { name: /Shared/ })).toBeVisible();
});
