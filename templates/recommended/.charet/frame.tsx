import { startFrame } from "@charet/sdk/frame";
import { createMemoryRouter, RouterProvider } from "react-router";
import "@/index.css";
import project from "./project.tsx";

void startFrame({
  project,
  // The app is loaded here, once the mocks run: an API client made while its modules load would
  // keep the real `fetch` and skip them.
  renderScreen: async (_screen, route) => {
    const { routes } = await import("@/routes");
    return <RouterProvider router={createMemoryRouter(routes, { initialEntries: [route] })} />;
  },
  wrapStory: (node) => <div className="p-6">{node}</div>,
});
