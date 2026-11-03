import assert from "node:assert/strict";
import { test } from "node:test";
import { defineProject, type ProjectDef, routeWithQuery, toManifest, validate } from "./project.ts";

const base: ProjectDef = {
  title: "Shop",
  pages: [
    { id: "cart", route: "/cart", title: "Cart" },
    { id: "pay", route: "/pay", title: "Pay" },
  ],
  screens: [
    { id: "cart-empty", page: "cart", title: "Empty cart" },
    {
      id: "cart-full",
      page: "cart",
      title: "Full cart",
      route: "/cart?demo=1",
      size: { w: 390, h: 844 },
    },
    { id: "pay-form", page: "pay", title: "Payment", setup: async () => {} },
  ],
  connections: [{ from: "cart-full", to: "pay-form", label: "Checkout" }],
  flows: [
    {
      id: "checkout",
      title: "Checkout",
      note: "pay for a full cart",
      steps: [
        "cart-full",
        { id: "pay-form", title: "Pay by card", note: "card form" },
        { id: "pay-done", title: "Paid", note: "receipt" },
        "refund",
      ],
    },
  ],
  stories: [{ id: "button-primary", component: "Button", title: "Primary", render: () => null }],
};

test("a valid project has no problems", () => {
  assert.deepEqual(validate(base), []);
  assert.equal(defineProject(base), base);
});

test("reports every problem with its id", () => {
  const broken: ProjectDef = {
    ...base,
    pages: [...base.pages, { id: "cart", route: "nope", title: "Dup" }],
    screens: [...base.screens, { id: "Bad Id", page: "missing", title: "x", size: { w: 0, h: 1 } }],
    connections: [{ from: "cart-full", to: "ghost" }],
    flows: [{ id: "empty", title: "Empty", steps: [] }],
  };
  const problems = validate(broken);
  for (const expected of [
    'page id "cart" is used twice',
    'page "cart" route must start with /',
    'screen id "Bad Id" must be lowercase',
    'screen "Bad Id" is on unknown page "missing"',
    'screen "Bad Id" size must be positive',
    'names unknown screen "ghost"',
    'flow "empty" has no steps',
  ]) {
    assert.ok(
      problems.some((p) => p.includes(expected)),
      `missing: ${expected}\n${problems.join("\n")}`,
    );
  }
  assert.throws(() => defineProject(broken), /invalid charet project/);
});

test("the manifest fills defaults and marks planned steps", () => {
  const manifest = toManifest(base);
  assert.equal(manifest.manifestVersion, 1);
  assert.deepEqual(manifest.screens[0], {
    id: "cart-empty",
    page: "cart",
    title: "Empty cart",
    note: "",
    route: "/cart",
    size: { w: 1440, h: 900 },
    hasSetup: false,
  });
  assert.equal(manifest.screens[1]?.route, "/cart?demo=1");
  assert.equal(manifest.screens[2]?.hasSetup, true);
  assert.deepEqual(manifest.connections, [
    { from: "cart-full", to: "pay-form", label: "Checkout", kind: "click" },
  ]);
  assert.deepEqual(
    manifest.flows[0]?.steps.map((s) => [s.id, s.title, s.planned]),
    [
      ["cart-full", "Full cart", false],
      ["pay-form", "Pay by card", false],
      ["pay-done", "Paid", true],
      ["refund", "refund", true],
    ],
  );
  assert.equal(manifest.flows[0]?.note, "pay for a full cart");
  assert.equal(manifest.flows[0]?.steps[1]?.note, "card form");
  assert.equal(manifest.pages[0]?.note, "");
  assert.deepEqual(manifest.stories, [
    { id: "button-primary", component: "Button", title: "Primary", size: { w: 480, h: 320 } },
  ]);
  // Functions never reach the JSON.
  assert.doesNotMatch(JSON.stringify(manifest), /render|setup|mocks/);
});

test("a screen's query goes into the route for a memory router", () => {
  assert.equal(routeWithQuery("/targets", { tab: "configs" }), "/targets?tab=configs");
  // The route's own query stays; the screen's value wins; the hash stays.
  assert.equal(routeWithQuery("/runs?x=1", { tab: "a" }), "/runs?x=1&tab=a");
  assert.equal(routeWithQuery("/runs?tab=b", { tab: "a" }), "/runs?tab=a");
  assert.equal(routeWithQuery("/docs#top", { tab: "a" }), "/docs?tab=a#top");
  assert.equal(routeWithQuery("/q", { text: "a b&c" }), "/q?text=a+b%26c");
  // Nothing to add: the route as it was.
  assert.equal(routeWithQuery("/runs?x=1"), "/runs?x=1");
  assert.equal(routeWithQuery("/runs?x=1", {}), "/runs?x=1");
});
