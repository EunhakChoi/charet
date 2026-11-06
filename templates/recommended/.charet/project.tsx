import { defineProject, variants } from "@charet/sdk";
import { HttpResponse, http } from "msw";
import { TaskRow } from "@/components/TaskRow";
import { Button } from "@/components/ui/button";
import { sampleTasks, tasks } from "./mocks.ts";

export default defineProject({
  title: "Tasks",
  pages: [{ id: "tasks", route: "/", title: "Tasks" }],
  screens: [
    { id: "tasks-list", page: "tasks", title: "Task list", mocks: [tasks(sampleTasks)] },
    { id: "tasks-empty", page: "tasks", title: "No tasks", mocks: [tasks([])] },
    {
      id: "tasks-open",
      page: "tasks",
      title: "Open tasks only",
      mocks: [tasks(sampleTasks)],
      setup: (kit) => kit.click("button", "Open"),
    },
    {
      id: "tasks-error",
      page: "tasks",
      title: "Load failed",
      mocks: [http.get("/api/tasks", () => new HttpResponse(null, { status: 500 }))],
    },
  ],
  connections: [{ from: "tasks-list", to: "tasks-open", label: "Open" }],
  flows: [
    {
      id: "triage",
      title: "Triage tasks",
      steps: [
        "tasks-list",
        "tasks-open",
        { id: "task-detail", title: "Task detail", note: "side panel with the task history" },
      ],
    },
  ],
  // One story per combination: the Components view gets a picker for each prop.
  stories: [
    ...variants({
      component: "Button",
      props: { variant: ["default", "secondary", "outline"], size: ["sm", "default", "lg"] },
      render: (p) => (
        <Button variant={p.variant as "default"} size={p.size as "default"}>
          Save
        </Button>
      ),
      code: (p) => `<Button variant="${p.variant}" size="${p.size}">Save</Button>`,
    }),
    // A state that is not a combination of props is an ordinary story, listed beside the picker.
    {
      id: "button-loading",
      component: "Button",
      title: "Loading",
      render: () => <Button disabled>Saving…</Button>,
    },
    // A component that is not a plain building block: it gets default values (a sample task) and
    // its own group, so the Components view lists it apart from the base components.
    ...variants({
      component: "TaskRow",
      group: "Feature",
      props: { done: ["false", "true"] },
      render: (p) => (
        <ul className="w-80 divide-y divide-border rounded-md border border-border">
          <TaskRow task={{ id: 1, title: "Write the release notes", done: p.done === "true" }} />
        </ul>
      ),
      code: (p) =>
        `<TaskRow task={{ id: 1, title: "Write the release notes", done: ${p.done} }} />`,
    }),
  ],
});
