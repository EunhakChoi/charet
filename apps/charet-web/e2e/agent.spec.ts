// The web agent from a screen: point at an element, ask, follow the run, then review the draft it
// made. The scripted agent stands in for a model (no key, same container-less path as `none`).

import { expect, test } from "@playwright/test";
import { asOwner } from "./people.ts";

const base = () => process.env.CHARET_E2E_URL ?? "";

test.describe.configure({ mode: "serial" });
// The owner, as the admin link or the CLI would sign them in.
test.use({ extraHTTPHeaders: asOwner() });

test("asking the agent from a screen makes a draft the team can review", async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const captured = await request.post(`${base()}/api/projects/demo/captures`, {
    data: { targets: ["tasks-list"], themes: ["light"] },
  });
  expect(captured.ok()).toBe(true);
  await page.goto(`${base()}/p/demo/t/screen/tasks-list`);
  await page
    .getByRole("radiogroup", { name: "Side panel" })
    .getByRole("radio", { name: "Agent" })
    .click();
  await expect(page.getByText(/The agent works on a draft branch/)).toBeVisible();

  // What was pointed at goes along with the request.
  const capture = page.getByTestId("capture");
  await expect(capture.locator("img")).toBeVisible();
  const box = await capture.boundingBox();
  if (!box) throw new Error("no capture");
  await page.mouse.click(box.x + box.width * 0.3, box.y + box.height * 0.15);
  await expect(page.getByText(/^screen tasks-list ·/)).toBeVisible();

  await page
    .getByRole("textbox", { name: "Request to the agent" })
    .fill("write notes/agent.txt: from the web\nsay I added a note.");
  await page.getByRole("button", { name: "Ask" }).click();
  await expect(page.getByText("I added a note.", { exact: true })).toBeVisible({ timeout: 60_000 });
  await expect(
    page
      .getByTestId(/^agent-run-/)
      .getByText("Done", { exact: true })
      .first(),
  ).toBeVisible();

  // Leaving the screen and coming back: the panel finds the conversation again.
  await page.goto(`${base()}/p/demo/t/screen/tasks-list`);
  await page
    .getByRole("radiogroup", { name: "Side panel" })
    .getByRole("radio", { name: "Agent" })
    .click();
  await expect(page.getByText("I added a note.", { exact: true })).toBeVisible();

  const open = page.getByRole("link", { name: /Open draft #\d+/ });
  await open.click();
  await expect(page).toHaveURL(/\/p\/demo\/branches\/\d+$/);
  await expect(page.getByText("Draft", { exact: true })).toBeVisible();

  // The branch keeps the conversation and takes the next request.
  await page.getByRole("radio", { name: "Agent" }).click();
  await expect(page.getByText("I added a note.", { exact: true })).toBeVisible();
  await page.getByRole("textbox", { name: "Request to the agent" }).fill("say Anything else?");
  await page.getByRole("button", { name: "Ask" }).click();
  await expect(page.getByText("Anything else?", { exact: true })).toBeVisible({ timeout: 60_000 });

  await page.getByRole("radio", { name: "Code" }).click();
  await expect(page.getByText("notes/agent.txt").first()).toBeVisible();
  await page.getByRole("button", { name: "Request review" }).click();
  await expect(page.getByText("In review", { exact: true })).toBeVisible();
});

test("the panel does not keep a branch that was closed since", async ({ page, request }) => {
  test.setTimeout(180_000);
  await page.goto(`${base()}/p/demo/t/screen/tasks-list`);
  const side = page.getByRole("radiogroup", { name: "Side panel" });
  await side.getByRole("radio", { name: "Agent" }).click();
  await page
    .getByRole("textbox", { name: "Request to the agent" })
    .fill("write notes/closed.txt: soon closed\nsay Noted.");
  await page.getByRole("button", { name: "Ask" }).click();
  await expect(page.getByText("Noted.", { exact: true })).toBeVisible({ timeout: 60_000 });
  const link = await page
    .getByRole("link", { name: /Open draft #\d+/ })
    .first()
    .textContent();
  const number = /#(\d+)/.exec(link ?? "")?.[1];
  if (!number) throw new Error(`no draft number in ${link}`);

  // Closed while the person was elsewhere; back on the screen the panel offers a new draft, not
  // a branch the server would refuse.
  const archived = await request.post(`${base()}/api/projects/demo/branches/${number}/archive`);
  expect(archived.ok()).toBe(true);
  await page.goto(`${base()}/p/demo/t/screen/tasks-list`);
  await side.getByRole("radio", { name: "Agent" }).click();
  await expect(page.getByText(/The agent works on a draft branch/)).toBeVisible();
  await expect(page.getByText("Noted.", { exact: true })).toHaveCount(0);
});

test("admins see the web agent settings and its costs", async ({ page }) => {
  await page.goto(`${base()}/settings/agent`);
  const settings = page.getByRole("region", { name: "Default agent and models" });
  await expect(settings.getByText("On", { exact: true })).toBeVisible();
  await expect(settings.getByRole("textbox", { name: "Model for claude" })).toBeVisible();
  const costs = page.getByRole("region", { name: "Agent costs" });
  await expect(costs.getByRole("cell", { name: "Admin" })).toBeVisible();
});
