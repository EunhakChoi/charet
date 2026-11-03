// Runs in flight: what each one said so far, how it ended and what it cost.

import type { Prices } from "./config.ts";
import type { Launched, Launcher } from "./launch.ts";
import type { Finished, Progress, RunEvent, RunnerLine, RunStart } from "./protocol.ts";
import { scrub, secretsOf } from "./secrets.ts";

interface Run {
  events: RunEvent[];
  finished: Finished | null;
  error: string | null;
  tokensIn: number;
  tokensOut: number;
  costUsd: number | null;
  canceled: boolean;
  launched?: Launched;
  endedAt?: number;
}

/** Finished runs are kept this long for the server to read their end. */
const KEEP_MS = 60 * 60 * 1000;

export class Runs {
  private readonly runs = new Map<number, Run>();
  private readonly launcher: Launcher;
  private readonly options: { prices: Prices; timeoutMs: number };

  constructor(launcher: Launcher, options: { prices: Prices; timeoutMs: number }) {
    this.launcher = launcher;
    this.options = options;
  }

  /** Starts a run; a second start of a known run is a no-op (the server may retry). */
  async start(start: RunStart): Promise<void> {
    this.forget();
    if (this.runs.has(start.id)) return;
    const run: Run = {
      events: [],
      finished: null,
      error: null,
      tokensIn: 0,
      tokensOut: 0,
      costUsd: null,
      canceled: false,
    };
    this.runs.set(start.id, run);
    try {
      run.launched = await this.launcher.launch(start, (line) => this.line(run, line));
    } catch (error) {
      this.runs.delete(start.id);
      throw error;
    }
    const timer = setTimeout(() => {
      if (run.finished) return;
      run.error = `took longer than ${Math.round(this.options.timeoutMs / 60_000)} minutes`;
      run.launched?.stop();
    }, this.options.timeoutMs);
    timer.unref();
    void run.launched.exited.then((code) => {
      clearTimeout(timer);
      run.endedAt = Date.now();
      if (run.finished) return;
      if (run.canceled) {
        run.finished = "canceled";
      } else {
        run.finished = "failed";
        // Its stderr is not the runner's report: hide the credentials before cutting it.
        const stderr = scrub(run.launched?.stderr() ?? "", secretsOf(start)).slice(-500);
        run.error ??= `the run stopped (exit ${code})${stderr ? `: ${stderr}` : ""}`;
      }
    });
  }

  progress(id: number, after: number): Progress | undefined {
    const run = this.runs.get(id);
    if (!run) return undefined;
    return {
      events: run.events.slice(after),
      next: run.events.length,
      finished: run.finished,
      error: run.error,
      tokensIn: run.tokensIn,
      tokensOut: run.tokensOut,
      costUsd: run.costUsd,
    };
  }

  /** Asks a run to stop; it saves what it made first. False when unknown. */
  cancel(id: number): boolean {
    const run = this.runs.get(id);
    if (!run) return false;
    if (!run.finished) {
      run.canceled = true;
      run.launched?.stop();
    }
    return true;
  }

  /** Runs still going. */
  active(): number {
    return [...this.runs.values()].filter((r) => !r.finished).length;
  }

  private line(run: Run, text: string): void {
    let line: RunnerLine;
    try {
      line = JSON.parse(text) as RunnerLine;
    } catch {
      return;
    }
    switch (line.type) {
      case "event":
        run.events.push({
          kind: line.kind,
          text: String(line.text).slice(0, 20_000),
          ...(line.tool ? { tool: String(line.tool) } : {}),
        });
        break;
      case "usage":
        run.tokensIn = line.tokensIn;
        run.tokensOut = line.tokensOut;
        run.costUsd = line.costUsd ?? price(this.options.prices, line);
        break;
      case "result":
        if (run.finished) break;
        run.finished = run.canceled && line.state === "done" ? "canceled" : line.state;
        if (line.error) run.error = line.error;
        break;
    }
  }

  private forget(): void {
    const now = Date.now();
    for (const [id, run] of this.runs) {
      if (run.endedAt && now - run.endedAt > KEEP_MS) this.runs.delete(id);
    }
  }
}

/** Cost from the price table, when the run's model is in it. */
export function price(
  prices: Prices,
  usage: { tokensIn: number; tokensOut: number; model?: string; cachedIn?: number },
): number | null {
  const entry = usage.model ? prices[usage.model] : undefined;
  if (!entry) return null;
  const cached = Math.min(usage.cachedIn ?? 0, usage.tokensIn);
  const cost =
    ((usage.tokensIn - cached) * entry.input +
      cached * (entry.cachedInput ?? entry.input) +
      usage.tokensOut * entry.output) /
    1_000_000;
  return Math.round(cost * 1e6) / 1e6;
}
