import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const root = fileURLToPath(new URL("..", import.meta.url));

test("a preview build has the app, the frame, the manifest and source locations", async () => {
  const outDir = mkdtempSync(join(tmpdir(), "charet-template-"));
  process.env.CHARET_PREVIEW = "1";
  try {
    await build({ root, logLevel: "error", build: { outDir, emptyOutDir: true } });
    assert.ok(existsSync(join(outDir, "index.html")));
    assert.ok(existsSync(join(outDir, ".charet/frame.html")));
    assert.ok(existsSync(join(outDir, "mockServiceWorker.js")));

    const manifest = JSON.parse(readFileSync(join(outDir, ".charet/manifest.json"), "utf8"));
    assert.equal(manifest.manifestVersion, 1);
    assert.deepEqual(
      manifest.screens.map((s: { id: string }) => s.id),
      ["tasks-list", "tasks-empty", "tasks-open", "tasks-error"],
    );
    assert.equal(manifest.flows[0].steps[2].planned, true);
    assert.equal(manifest.stories.length, 9);

    const js = readdirSync(join(outDir, "assets"))
      .filter((f) => f.endsWith(".js"))
      .map((f) => readFileSync(join(outDir, "assets", f), "utf8"))
      .join("\n");
    assert.match(js, /src\/pages\/tasks\.tsx:\d+:\d+/);
    // The frame's own files are not marked, and component names survive for the element map.
    assert.doesNotMatch(js, /\.charet\/frame\.tsx:\d+/);
    assert.match(js, /function Button\(/);
  } finally {
    delete process.env.CHARET_PREVIEW;
    rmSync(outDir, { recursive: true, force: true });
  }
});

test("a plain build of the app has no charet parts", async () => {
  const outDir = mkdtempSync(join(tmpdir(), "charet-template-"));
  try {
    await build({ root, logLevel: "error", build: { outDir, emptyOutDir: true } });
    assert.ok(existsSync(join(outDir, "index.html")));
    assert.ok(!existsSync(join(outDir, ".charet")));
    assert.ok(!existsSync(join(outDir, "mockServiceWorker.js")));
    const js = readdirSync(join(outDir, "assets"))
      .filter((f) => f.endsWith(".js"))
      .map((f) => readFileSync(join(outDir, "assets", f), "utf8"))
      .join("\n");
    assert.doesNotMatch(js, /data-src/);
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
});
