import { useEffect, useState } from "react";
import { fetchTasks, type Task } from "@/api";
import { TaskRow } from "@/components/TaskRow";
import { Button } from "@/components/ui/button";

export function TasksPage() {
  const [tasks, setTasks] = useState<Task[] | undefined>();
  const [error, setError] = useState<string | undefined>();
  const [filter, setFilter] = useState<"all" | "open">("all");

  useEffect(() => {
    fetchTasks().then(setTasks, (e: unknown) => setError(String(e)));
  }, []);

  const shown = tasks?.filter((t) => filter === "all" || !t.done);
  return (
    <main className="mx-auto max-w-xl p-8">
      <header className="mb-6 flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Tasks</h1>
        <div className="flex gap-2">
          <Button
            variant={filter === "all" ? "default" : "outline"}
            onClick={() => setFilter("all")}
          >
            All
          </Button>
          <Button
            variant={filter === "open" ? "default" : "outline"}
            onClick={() => setFilter("open")}
          >
            Open
          </Button>
        </div>
      </header>
      {error && <p className="text-sm text-red-600">{error}</p>}
      {!tasks && !error && <p className="text-sm text-muted-foreground">Loading…</p>}
      {shown && shown.length === 0 && (
        <p className="text-sm text-muted-foreground">Nothing to do.</p>
      )}
      <ul className="divide-y divide-border rounded-md border border-border">
        {shown?.map((task) => (
          <TaskRow key={task.id} task={task} />
        ))}
      </ul>
    </main>
  );
}
