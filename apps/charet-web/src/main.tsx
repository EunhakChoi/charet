import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { createBrowserRouter, Navigate, RouterProvider } from "react-router";
import { AuthGate } from "@/components/AuthGate";
import { api } from "@/lib/api";
import { LocaleProvider } from "@/lib/i18n";
import { Branch } from "@/pages/Branch";
import { Branches } from "@/pages/Branches";
import { Code } from "@/pages/Code";
import { Components } from "@/pages/Components";
import { Flows } from "@/pages/Flows";
import { ProjectLayout } from "@/pages/ProjectLayout";
import { Projects } from "@/pages/Projects";
import { Screens } from "@/pages/Screens";
import { Settings } from "@/pages/Settings";
import { Target } from "@/pages/Target";
import { Versions } from "@/pages/Versions";
import "./index.css";

const router = createBrowserRouter([
  {
    path: "/",
    element: (
      <AuthGate>
        <Projects />
      </AuthGate>
    ),
  },
  {
    path: "/settings/:tab?",
    element: (
      <AuthGate>
        <Settings />
      </AuthGate>
    ),
  },
  // The page was called Team before it held more than people.
  { path: "/team", element: <Navigate to="/settings/members" replace /> },
  {
    path: "/p/:key",
    element: (
      <AuthGate>
        <ProjectLayout />
      </AuthGate>
    ),
    children: [
      { index: true, element: <Navigate to="screens" replace /> },
      { path: "screens", element: <Screens /> },
      { path: "flows", element: <Flows /> },
      { path: "components", element: <Components /> },
      { path: "branches", element: <Branches /> },
      { path: "branches/:number", element: <Branch /> },
      { path: "versions", element: <Versions /> },
      { path: "code", element: <Code /> },
      { path: "t/:kind/:target", element: <Target /> },
    ],
  },
]);

const queries = new QueryClient({
  defaultOptions: { queries: { staleTime: 5_000, refetchOnWindowFocus: true } },
});

/**
 * A link an admin handed over (`…/?t=<token>`) signs this browser in. The token leaves the
 * address bar first, so it stays out of history and screenshots.
 */
async function enterLinkToken() {
  const url = new URL(window.location.href);
  const token = url.searchParams.get("t");
  if (!token) return;
  url.searchParams.delete("t");
  window.history.replaceState(window.history.state, "", url);
  await api.POST("/api/auth/token", { body: { token } }).catch(() => undefined);
}

const root = document.getElementById("root");
if (!root) throw new Error("no #root element");
void enterLinkToken().finally(() =>
  createRoot(root).render(
    <StrictMode>
      <QueryClientProvider client={queries}>
        <LocaleProvider>
          <RouterProvider router={router} />
        </LocaleProvider>
      </QueryClientProvider>
    </StrictMode>,
  ),
);
