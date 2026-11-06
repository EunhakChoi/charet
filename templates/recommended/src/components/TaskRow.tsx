import type { Task } from "@/api";

/** One row of the task list. */
export function TaskRow({ task }: { task: Task }) {
  return (
    <li className="flex items-center gap-3 px-4 py-3">
      <span className={task.done ? "text-muted-foreground line-through" : ""}>{task.title}</span>
    </li>
  );
}
