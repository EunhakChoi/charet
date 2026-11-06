import assert from "node:assert/strict";
import { test } from "node:test";
import type { Manifest } from "./api";
import {
  byComponent,
  firstStory,
  pickProp,
  plainStories,
  propChoices,
  propsOf,
} from "./gallery.ts";

type Story = Manifest["stories"][number];
const size = { w: 480, h: 320 };
const story = (id: string, component: string, props?: Record<string, string>): Story => ({
  id,
  component,
  title: id,
  size,
  ...(props ? { props: Object.entries(props).map(([name, value]) => ({ name, value })) } : {}),
});

// A matrix with a gap: there is no outline + lg.
const button = [
  story("b-d-sm", "Button", { variant: "default", size: "sm" }),
  story("b-d-lg", "Button", { variant: "default", size: "lg" }),
  story("b-o-sm", "Button", { variant: "outline", size: "sm" }),
];

test("stories are grouped by component in the order they appear", () => {
  const groups = byComponent(
    [button[0], story("c", "Card"), button[1]].filter((s): s is Story => !!s),
  );
  assert.deepEqual(
    groups.map((g) => [g.name, g.stories.map((s) => s.id)]),
    [
      ["Button", ["b-d-sm", "b-d-lg"]],
      ["Card", ["c"]],
    ],
  );
});

test("props are listed with their values in order, and plain stories have none", () => {
  assert.deepEqual(propChoices(button), [
    { name: "variant", values: ["default", "outline"] },
    { name: "size", values: ["sm", "lg"] },
  ]);
  assert.deepEqual(propChoices([story("a", "Card"), story("b", "Card")]), []);
});

test("picking a value keeps the other props when a story has them, else the closest", () => {
  const [dsm, dlg, osm] = button as [Story, Story, Story];
  assert.equal(pickProp(button, dsm, "variant", "outline"), osm);
  assert.equal(pickProp(button, dlg, "variant", "outline"), osm, "no outline + lg: keep outline");
  assert.equal(pickProp(button, osm, "size", "lg"), dlg, "no outline + lg: keep lg");
  assert.equal(pickProp(button, dsm, "size", "lg"), dlg);
  assert.equal(pickProp(button, dsm, "size", "xl"), dsm, "an unknown value changes nothing");
  const tie = [
    story("t1", "Tag", { tone: "warm", size: "1" }),
    story("t2", "Tag", { tone: "warm", size: "2" }),
    story("t3", "Tag", { tone: "cold", size: "3" }),
  ] as [Story, Story, Story];
  assert.equal(pickProp(tie, tie[2], "tone", "warm"), tie[0], "nothing is kept: the first wins");
});

test("the first story is the one built from default values", () => {
  assert.equal(firstStory(button)?.id, "b-d-sm");
  const shifted = [
    story("x1", "Badge", { variant: "warning" }),
    story("x2", "Badge", { variant: "default" }),
  ];
  assert.equal(firstStory(shifted)?.id, "x2");
  assert.equal(firstStory([story("p", "Card"), story("q", "Card")])?.id, "p");
});

test("plain stories stay listed next to a component's props", () => {
  const mixed = [...button, story("b-loading", "Button")];
  assert.deepEqual(
    plainStories(mixed).map((s) => s.id),
    ["b-loading"],
  );
  assert.deepEqual(plainStories(button), []);
  // Picking a prop from the plain state lands on a story that sets it.
  assert.equal(pickProp(mixed, mixed[3] as Story, "variant", "outline").id, "b-o-sm");
});

test("a manifest from the first SDK with props (an object) still reads", () => {
  const old = { id: "o", component: "Button", title: "o", size, props: { variant: "outline" } };
  const stories = [old as unknown as Story, story("d", "Button", { variant: "default" })];
  assert.deepEqual(propsOf(stories[0] as Story), [{ name: "variant", value: "outline" }]);
  assert.deepEqual(propChoices(stories), [{ name: "variant", values: ["outline", "default"] }]);
  assert.equal(pickProp(stories, stories[1] as Story, "variant", "outline").id, "o");
  assert.deepEqual(plainStories(stories), []);
});
