import { createServer, type IncomingHttpHeaders } from "node:http";
import type { AddressInfo } from "node:net";
import { expect, test } from "@playwright/test";
import { addMember, adminToken, base, enter } from "./people.ts";

// Notifications and webhooks on the public server (`stack.ts`).

test("a new comment rings the bell and opens on the comment", async ({ browser }) => {
  const boss = await (await browser.newContext()).newPage();
  await enter(boss, adminToken());
  const annToken = await addMember(boss, "Ann", "editor");
  await boss.goto(`${base()}/p/demo/screens`);

  const ann = await (await browser.newContext()).newPage();
  await enter(ann, annToken);
  const added = await ann.request.post(`${base()}/api/projects/demo/comments`, {
    headers: { "x-charet-client": "e2e" },
    data: { kind: "screen", target: "tasks-list", at: { x: 20, y: 20 }, body: "Ping from Ann" },
  });
  expect(added.status()).toBe(201);

  // No reload: the room nudges Boss's inbox.
  const bell = boss.getByRole("button", { name: "Notifications (1 unread)" });
  await expect(bell).toBeVisible();
  await bell.click();
  const panel = boss.getByRole("region", { name: "Notifications" });
  await panel.getByRole("button", { name: /Ann commented on tasks-list/ }).click();
  await expect(boss).toHaveURL(/\/p\/demo\/t\/screen\/tasks-list\?comment=\d+$/);
  await expect(boss.getByText("Ping from Ann")).toBeVisible();
  await expect(boss.getByRole("button", { name: "Notifications (0 unread)" })).toBeVisible();
});

test("an admin adds a webhook and a test delivery arrives signed", async ({ page }) => {
  const received: { headers: IncomingHttpHeaders; body: string }[] = [];
  const receiver = createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => {
      body += chunk;
    });
    req.on("end", () => {
      received.push({ headers: req.headers, body });
      res.writeHead(204);
      res.end();
    });
  });
  await new Promise<void>((done) => receiver.listen(0, "127.0.0.1", done));
  const url = `http://127.0.0.1:${(receiver.address() as AddressInfo).port}/hook`;
  try {
    await enter(page, adminToken());
    await page.getByRole("link", { name: "Settings" }).click();
    await page.getByRole("link", { name: "Webhooks" }).click();
    const hooks = page.getByRole("region", { name: "Webhooks" });
    await hooks.getByRole("textbox", { name: "Webhook address" }).fill(url);
    await hooks.getByRole("checkbox", { name: "comment.added" }).check();
    await hooks.getByRole("button", { name: "Add webhook" }).click();
    const secret = await hooks.getByRole("textbox", { name: "Signing secret" }).inputValue();
    expect(secret).toMatch(/^whsec_/);
    await hooks.getByRole("button", { name: "Send test" }).click();
    await expect(hooks.getByText("204", { exact: true })).toBeVisible();
    expect(received).toHaveLength(1);
    expect(received[0]?.headers["x-charet-event"]).toBe("ping");
    expect(received[0]?.headers["x-charet-signature"]).toMatch(/^sha256=[0-9a-f]{64}$/);
    await hooks.getByRole("button", { name: `Remove ${url}` }).click();
    await expect(hooks.getByText(/No webhooks/)).toBeVisible();
  } finally {
    receiver.close();
  }
});
