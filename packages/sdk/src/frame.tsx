// The frame renders one screen or one story of a charet project, with mock APIs, for the charet to
// show and capture. URL: `.charet/frame.html?screen=<id>&theme=light|dark` or `?story=<id>`.
// When the page is settled it sets `<html data-charet-ready>`; the renderer waits for that.

import type { RequestHandler } from "msw";
import { type ReactNode, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { type ProjectDef, routeWithQuery, type ScreenDef, type SetupKit } from "./project.ts";

export interface FrameOptions {
  project: ProjectDef<RequestHandler, ReactNode>;
  /**
   * Renders the app at `route` (e.g. a memory router over the app's routes, inside its providers).
   * The route carries the screen's `query`. May be async: load the app's modules in here
   * (`await import("@/routes")`), after the mocks are running. A client an app builds while its
   * modules load (a module-level `createClient`, `const f = fetch`) keeps the `fetch` it saw then,
   * so imported at the top of the frame it would skip the mocks.
   */
  renderScreen: (
    screen: ScreenDef<RequestHandler>,
    route: string,
  ) => ReactNode | Promise<ReactNode>;
  /** Wraps stories, e.g. with the app's theme provider. */
  wrapStory?: (node: ReactNode) => ReactNode;
  /** Root element id (default `root`). */
  rootId?: string;
}

/** Logged when a screen calls an API with no handler; the charet reports it as a missing mock. */
export const MISSING_MOCK = "[charet] missing mock";

export async function startFrame(options: FrameOptions): Promise<void> {
  const params = new URLSearchParams(location.search);
  const theme = params.get("theme") === "dark" ? "dark" : "light";
  document.documentElement.classList.toggle("dark", theme === "dark");
  document.documentElement.dataset.theme = theme;

  const container = document.getElementById(options.rootId ?? "root");
  if (!container) throw new Error(`no #${options.rootId ?? "root"} element`);
  const { project } = options;
  const screenId = params.get("screen");
  const storyId = params.get("story");

  let node: ReactNode;
  let screen: ScreenDef<RequestHandler> | undefined;
  if (screenId) {
    screen = project.screens.find((s) => s.id === screenId);
    if (!screen) return fail(container, `unknown screen "${screenId}"`);
    const page = project.pages.find((p) => p.id === screen?.page);
    await startMocks([...(screen.mocks ?? []), ...(project.mocks ?? [])]);
    if (screen.query) {
      const url = new URL(location.href);
      for (const [key, value] of Object.entries(screen.query)) url.searchParams.set(key, value);
      history.replaceState(history.state, "", url);
    }
    node = await options.renderScreen(
      screen,
      routeWithQuery(screen.route ?? page?.route ?? "/", screen.query),
    );
  } else if (storyId) {
    const story = project.stories?.find((s) => s.id === storyId);
    if (!story) return fail(container, `unknown story "${storyId}"`);
    const handlers = [...(story.mocks ?? []), ...(project.mocks ?? [])];
    if (handlers.length > 0) await startMocks(handlers);
    node = (options.wrapStory ?? ((n) => n))(story.render());
  } else {
    return fail(container, "pass ?screen=<id> or ?story=<id>");
  }

  createRoot(container).render(<StrictMode>{node}</StrictMode>);
  await settle();
  if (screen?.setup) {
    await screen.setup(setupKit(container));
    await settle();
  }
  document.documentElement.dataset.charetReady = "true";
}

/** Asset requests of the page itself are not API calls; a missing handler for them is fine. */
function isAsset(url: URL): boolean {
  return url.host === location.host && /\.\w+$/.test(url.pathname);
}

/** Mocks answer the page's fetch and XHR calls in the page itself. No service worker: previews
 * may run in an opaque origin (one address for the tool and previews), where none can register,
 * and their URL changes with every access token. */
async function startMocks(handlers: RequestHandler[]): Promise<void> {
  await startInterceptors(handlers);
}

async function startInterceptors(handlers: RequestHandler[]): Promise<void> {
  const [
    { BatchInterceptor },
    { FetchInterceptor },
    { XMLHttpRequestInterceptor },
    { getResponse },
  ] = await Promise.all([
    import("@mswjs/interceptors"),
    import("@mswjs/interceptors/fetch"),
    import("@mswjs/interceptors/XMLHttpRequest"),
    import("msw"),
  ]);
  const interceptor = new BatchInterceptor({
    name: "charet-frame",
    interceptors: [new FetchInterceptor(), new XMLHttpRequestInterceptor()],
  });
  interceptor.apply();
  interceptor.on("request", async ({ request, controller }) => {
    const response = await getResponse(handlers, request);
    // `passthrough()` lets the request go to the network.
    if (response && response.headers.get("x-msw-intention") !== "passthrough") {
      controller.respondWith(response);
      return;
    }
    if (response) return;
    const url = new URL(request.url);
    if (!isAsset(url))
      console.warn(`${MISSING_MOCK} ${request.method} ${url.pathname}${url.search}`);
  });
}

function fail(container: HTMLElement, message: string): void {
  container.textContent = `charet: ${message}`;
  console.error(`[charet] ${message}`);
  document.documentElement.dataset.charetReady = "error";
}

/** Waits for two frames and for fonts, so the first paint after data arrives is in. */
async function settle(): Promise<void> {
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  await document.fonts?.ready;
  await new Promise((r) => setTimeout(r, 50));
}

function setupKit(root: HTMLElement): SetupKit {
  const find = async (selector: string, text?: string): Promise<HTMLElement> => {
    for (let i = 0; i < 100; i++) {
      // Overlays (dialogs, menus) render outside the app root, so search the whole document.
      const match = [...document.querySelectorAll<HTMLElement>(selector)].find(
        (el) =>
          text === undefined ||
          el.textContent?.replace(/\s+/g, " ").trim() === text ||
          el.getAttribute("aria-label") === text,
      );
      if (match) return match;
      await new Promise((r) => setTimeout(r, 50));
    }
    throw new Error(`setup: no ${selector}${text ? ` with text "${text}"` : ""}`);
  };
  return {
    root,
    find,
    async click(selector, text) {
      const el = await find(selector, text);
      // Radix triggers (Select, DropdownMenu) open on pointerdown: send what a real click sends.
      const init = { bubbles: true, cancelable: true, button: 0, pointerType: "mouse" };
      el.dispatchEvent(new PointerEvent("pointerdown", init));
      el.dispatchEvent(new MouseEvent("mousedown", init));
      el.dispatchEvent(new PointerEvent("pointerup", init));
      el.dispatchEvent(new MouseEvent("mouseup", init));
      el.click();
    },
    async fill(selector, value) {
      const input = (await find(selector)) as HTMLInputElement;
      const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(input), "value")?.set;
      setter?.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
    },
    wait: (ms) => new Promise((r) => setTimeout(r, ms)),
  };
}
