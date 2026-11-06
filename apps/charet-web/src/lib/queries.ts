// Server state for the web tool (TanStack Query over the typed API client).

import type { Schemas } from "@charet/api-client";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  type AgentKind,
  type AgentRun,
  api,
  type Comment,
  type Element,
  HttpError,
  type ProviderId,
  type Role,
  type ShareMode,
  type TargetKind,
  type Theme,
  unwrap,
  unwrapEmpty,
  type VisitorAccess,
} from "./api.ts";

const path = (key: string) => ({ params: { path: { key } } });

export function useProjects() {
  return useQuery({ queryKey: ["projects"], queryFn: () => unwrap(api.GET("/api/projects")) });
}

export function useProject(key: string, enabled = true) {
  return useQuery({
    queryKey: ["project", key],
    enabled,
    queryFn: () => unwrap(api.GET("/api/projects/{key}", path(key))),
  });
}

export function useVersions(key: string) {
  return useQuery({
    queryKey: ["versions", key],
    queryFn: () => unwrap(api.GET("/api/projects/{key}/versions", path(key))),
  });
}

/** The preview of `main` (or of `rev`); polls while it builds. */
export function usePreview(key: string, rev?: string) {
  return useQuery({
    queryKey: ["preview", key, rev ?? "main"],
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/preview", {
          params: { path: { key }, query: rev ? { rev } : {} },
        }),
      ),
    refetchInterval: (query) => {
      const state = query.state.data?.state;
      return state === "queued" || state === "running" ? 1500 : false;
    },
  });
}

/** Pages, screens, flows and stories of `main` (or of `rev`); retried while the preview builds. */
export function useManifest(key: string, rev?: string) {
  return useQuery({
    queryKey: ["manifest", key, rev ?? "main"],
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/manifest", {
          params: { path: { key }, query: rev ? { rev } : {} },
        }),
      ),
    retry: (count, error) => error instanceof HttpError && error.code === "pending" && count < 400,
    retryDelay: 1500,
    placeholderData: keepPreviousData,
  });
}

export function useCaptures(key: string) {
  return useQuery({
    queryKey: ["captures", key],
    queryFn: () => unwrap(api.GET("/api/projects/{key}/captures", path(key))),
  });
}

/** The components the project's screens use, read from their captures in `theme`. */
export function useComponents(key: string, theme: Theme) {
  return useQuery({
    queryKey: ["components", key, theme],
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/components", {
          params: { path: { key }, query: { theme } },
        }),
      ),
    // The screens are listed from the manifest, which waits for the build like useManifest does.
    retry: (count, error) => error instanceof HttpError && error.code === "pending" && count < 400,
    retryDelay: 1500,
  });
}

export function useElements(
  key: string,
  kind: TargetKind,
  target: string,
  theme: Theme,
  enabled: boolean,
) {
  return useQuery({
    queryKey: ["elements", key, kind, target, theme],
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/captures/{kind}/{target}/{theme}/elements", {
          params: { path: { key, kind, target, theme } },
        }),
      ),
    enabled,
  });
}

export function useComments(key: string) {
  return useQuery({
    queryKey: ["comments", key],
    queryFn: () => unwrap(api.GET("/api/projects/{key}/comments", path(key))),
  });
}

export function useFiles(key: string) {
  return useQuery({
    queryKey: ["files", key],
    queryFn: () => unwrap(api.GET("/api/projects/{key}/files", path(key))),
  });
}

/** A file of `main` as text (`undefined` when it is not UTF-8). */
export function useFile(key: string, file: string | undefined) {
  return useQuery({
    queryKey: ["file", key, file],
    enabled: !!file,
    retry: (count, error) => !(error instanceof HttpError && error.status < 500) && count < 3,
    queryFn: async () => {
      const response = await fetch(
        `/api/projects/${encodeURIComponent(key)}/file?path=${encodeURIComponent(file ?? "")}`,
      );
      if (!response.ok) {
        const body = (await response.json().catch(() => ({}))) as {
          code?: string;
          message?: string;
        };
        throw new HttpError(
          response.status,
          body.code ?? "error",
          body.message ?? response.statusText,
        );
      }
      const text = response.headers.get("content-type")?.startsWith("text/")
        ? await response.text()
        : undefined;
      return { text };
    },
  });
}

/** The app directory inside the repository (`charet.json` `app`), `""` for the root. */
export function useAppDir(key: string) {
  const file = useFile(key, "charet.json");
  if (!file.data?.text) return "";
  try {
    const app = (JSON.parse(file.data.text) as { app?: unknown }).app;
    return typeof app === "string" && app !== "." ? app : "";
  } catch {
    return "";
  }
}

export function useCapture(key: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { targets?: string[]; themes?: Theme[]; force?: boolean }) =>
      unwrap(api.POST("/api/projects/{key}/captures", { ...path(key), body: input })),
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ["captures", key] });
      void client.invalidateQueries({ queryKey: ["components", key] });
      void client.invalidateQueries({ queryKey: ["elements", key] });
      void client.invalidateQueries({ queryKey: ["comments", key] });
    },
  });
}

export function useRetryBuild(key: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/preview", {
          params: { path: { key }, query: { retry: true } },
        }),
      ),
    onSuccess: (preview) => {
      client.setQueryData(["preview", key], preview);
      void client.invalidateQueries({ queryKey: ["manifest", key] });
    },
  });
}

export type NewComment = {
  kind: TargetKind;
  target: string;
  theme: Theme;
  at: { x: number; y: number } | { selector: string };
  body: string;
  /** On this branch's screen instead of main's. */
  branch?: number;
};

/** Comments on a branch's screens. */
export function useBranchComments(key: string, number: number) {
  return useQuery({
    queryKey: ["comments", key, "branch", number],
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/comments", {
          params: { path: { key }, query: { branch: number } },
        }),
      ),
  });
}

export function useAddComment(key: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: NewComment) =>
      unwrap(api.POST("/api/projects/{key}/comments", { ...path(key), body: input })),
    onSuccess: () => client.invalidateQueries({ queryKey: ["comments", key] }),
  });
}

export function useAnswer(key: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async (input: {
      comment: Comment;
      action: "replies" | "resolve" | "reopen";
      body: string;
    }) => {
      const params = {
        params: { path: { key, id: input.comment.id } },
        body: { rev: input.comment.rev, body: input.body },
      };
      switch (input.action) {
        case "replies":
          return unwrap(api.POST("/api/projects/{key}/comments/{id}/replies", params));
        case "resolve":
          return unwrap(api.POST("/api/projects/{key}/comments/{id}/resolve", params));
        case "reopen":
          return unwrap(api.POST("/api/projects/{key}/comments/{id}/reopen", params));
      }
    },
    // A stale write (`changed`) also means the list is out of date.
    onSettled: () => client.invalidateQueries({ queryKey: ["comments", key] }),
  });
}

/** Who is looking: this machine's admin, a member, or a visitor. */
export function useMe() {
  return useQuery({
    queryKey: ["me"],
    retry: (count, error) => !(error instanceof HttpError && error.status < 500) && count < 3,
    queryFn: () => unwrap(api.GET("/api/auth/me")),
  });
}

/** Comes in with a member's token; a fresh page load picks it up everywhere. */
export function useEnterToken() {
  return useMutation({
    mutationFn: (token: string) => unwrap(api.POST("/api/auth/token", { body: { token } })),
  });
}

export function useSignOut() {
  return useMutation({
    mutationFn: () => unwrapEmpty(api.POST("/api/auth/logout")).catch(() => undefined),
    onSettled: () => window.location.assign("/"),
  });
}

/** Members with their tokens, for the admin's Team page. */
export function useUsers(enabled = true) {
  return useQuery({
    queryKey: ["admin", "users"],
    queryFn: () => unwrap(api.GET("/api/admin/users")),
    enabled,
    refetchInterval: 10_000,
  });
}

export function useAccess(enabled: boolean) {
  return useQuery({
    queryKey: ["admin", "access"],
    queryFn: () => unwrap(api.GET("/api/admin/access")),
    enabled,
  });
}

export function useAdminActions() {
  const client = useQueryClient();
  const settled = () => client.invalidateQueries({ queryKey: ["admin"] });
  const user = (id: string) => ({ params: { path: { id } } });
  return {
    add: useMutation({
      mutationFn: (body: { name: string; role: Role }) =>
        unwrap(api.POST("/api/admin/users", { body })),
      onSettled: settled,
    }),
    change: useMutation({
      mutationFn: ({ id, ...body }: { id: string; name?: string; role?: Role }) =>
        unwrap(api.PATCH("/api/admin/users/{id}", { ...user(id), body })),
      onSettled: () => {
        settled();
        // Renaming the owner renames this machine's admin.
        client.invalidateQueries({ queryKey: ["me"] });
      },
    }),
    reissue: useMutation({
      mutationFn: (id: string) => unwrap(api.POST("/api/admin/users/{id}/token", user(id))),
      onSettled: settled,
    }),
    remove: useMutation({
      mutationFn: (id: string) => unwrapEmpty(api.DELETE("/api/admin/users/{id}", user(id))),
      onSettled: settled,
    }),
    visitors: useMutation({
      mutationFn: (visitors: VisitorAccess) =>
        unwrap(api.PUT("/api/admin/access", { body: { visitors } })),
      onSettled: () => {
        settled();
        client.invalidateQueries({ queryKey: ["project"] });
        client.invalidateQueries({ queryKey: ["projects"] });
      },
    }),
  };
}

export function useProjectAccess(key: string, enabled: boolean) {
  return useQuery({
    queryKey: ["members", key],
    enabled,
    queryFn: () => unwrap(api.GET("/api/projects/{key}/members", path(key))),
  });
}

export function useProjectAccessActions(key: string) {
  const client = useQueryClient();
  return {
    visibility: useMutation({
      mutationFn: (visibility: "team" | "private") =>
        unwrap(api.PATCH("/api/projects/{key}", { ...path(key), body: { visibility } })),
      onSettled: () => {
        client.invalidateQueries({ queryKey: ["project", key] });
        client.invalidateQueries({ queryKey: ["projects"] });
      },
    }),
    visitors: useMutation({
      mutationFn: (visitors: VisitorAccess | null) =>
        unwrap(api.PUT("/api/projects/{key}/visitors", { ...path(key), body: { visitors } })),
      onSettled: () => {
        client.invalidateQueries({ queryKey: ["project", key] });
        client.invalidateQueries({ queryKey: ["projects"] });
      },
    }),
    member: useMutation({
      mutationFn: ({ user, member }: { user: string; member: boolean }) => {
        const params = { params: { path: { key, user } } };
        return unwrapEmpty(
          member
            ? api.PUT("/api/projects/{key}/members/{user}", params)
            : api.DELETE("/api/projects/{key}/members/{user}", params),
        );
      },
      // The checkbox follows the click at once; the refetch confirms it.
      onMutate: async ({ user, member }) => {
        // A fetch still in flight would bring back the old list.
        await client.cancelQueries({ queryKey: ["members", key] });
        client.setQueryData<{ members: string[] }>(["members", key], (old) => {
          const rest = (old?.members ?? []).filter((id) => id !== user);
          return { members: member ? [...rest, user] : rest };
        });
      },
      onSettled: () => client.invalidateQueries({ queryKey: ["members", key] }),
    }),
  };
}

/** The signed-in member's inbox. Realtime nudges refetch it; the poll covers pages without a room. */
export function useInbox(enabled: boolean) {
  return useQuery({
    queryKey: ["notifications"],
    enabled,
    queryFn: () => unwrap(api.GET("/api/me/notifications", { params: { query: { limit: 30 } } })),
    refetchInterval: 60_000,
  });
}

export function useMarkRead() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (ids?: number[]) =>
      unwrap(api.POST("/api/me/notifications/read", { body: ids ? { ids } : {} })),
    onSettled: () => client.invalidateQueries({ queryKey: ["notifications"] }),
  });
}

export function useWebhooks() {
  return useQuery({
    queryKey: ["admin", "webhooks"],
    queryFn: () => unwrap(api.GET("/api/admin/webhooks")),
  });
}

export function useWebhookActions() {
  const client = useQueryClient();
  const settled = () => client.invalidateQueries({ queryKey: ["admin", "webhooks"] });
  const hook = (id: string) => ({ params: { path: { id } } });
  return {
    create: useMutation({
      mutationFn: (body: { url: string; events: string[]; project?: string | null }) =>
        unwrap(api.POST("/api/admin/webhooks", { body })),
      onSettled: settled,
    }),
    remove: useMutation({
      mutationFn: (id: string) => unwrapEmpty(api.DELETE("/api/admin/webhooks/{id}", hook(id))),
      onSettled: settled,
    }),
    rotate: useMutation({
      mutationFn: (id: string) => unwrap(api.POST("/api/admin/webhooks/{id}/rotate", hook(id))),
      onSettled: settled,
    }),
    test: useMutation({
      mutationFn: (id: string) => unwrap(api.POST("/api/admin/webhooks/{id}/test", hook(id))),
      onSettled: settled,
    }),
  };
}

/** Versions, links and services (answers without a token). */
export function useSystem() {
  return useQuery({
    queryKey: ["system"],
    queryFn: () => unwrap(api.GET("/api/system")),
    staleTime: 60_000,
  });
}

export function useSharing(enabled: boolean) {
  return useQuery({
    queryKey: ["admin", "sharing"],
    queryFn: () => unwrap(api.GET("/api/admin/sharing")),
    enabled,
  });
}

export function useSharingActions() {
  const client = useQueryClient();
  const settled = () => {
    client.invalidateQueries({ queryKey: ["admin", "sharing"] });
    client.invalidateQueries({ queryKey: ["system"] });
  };
  return {
    start: useMutation({
      mutationFn: (body: { mode: ShareMode; url?: string; authtoken?: string; domain?: string }) =>
        unwrap(api.PUT("/api/admin/sharing", { body })),
      onSettled: settled,
    }),
    stop: useMutation({
      mutationFn: () => unwrap(api.DELETE("/api/admin/sharing")),
      onSettled: settled,
    }),
  };
}

export function useBranches(key: string) {
  return useQuery({
    queryKey: ["branches", key],
    queryFn: () => unwrap(api.GET("/api/projects/{key}/branches", path(key))),
  });
}

export function useBranch(key: string, number: number) {
  return useQuery({
    queryKey: ["branch", key, number],
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/branches/{number}", {
          params: { path: { key, number } },
        }),
      ),
  });
}

/** A file at `rev` as text (`undefined` when missing or not UTF-8). */
export function useFileAt(key: string, rev: string | undefined, file: string | undefined) {
  return useQuery({
    queryKey: ["file", key, rev, file],
    enabled: !!rev && !!file,
    queryFn: async () => {
      const response = await fetch(
        `/api/projects/${encodeURIComponent(key)}/file?rev=${encodeURIComponent(rev ?? "")}&path=${encodeURIComponent(file ?? "")}`,
      );
      if (response.status === 404) return { text: undefined };
      if (!response.ok) throw new HttpError(response.status, "error", response.statusText);
      const text = response.headers.get("content-type")?.startsWith("text/")
        ? await response.text()
        : undefined;
      return { text };
    },
  });
}

/** Review, merge, archive, reopen, duplicate and catch-up of branches; restoring versions. */
export function useBranchActions(key: string) {
  const client = useQueryClient();
  const settled = () => {
    void client.invalidateQueries({ queryKey: ["branches", key] });
    void client.invalidateQueries({ queryKey: ["branch", key] });
    void client.invalidateQueries({ queryKey: ["versions", key] });
  };
  const on = (number: number) => ({ params: { path: { key, number } } });
  return {
    review: useMutation({
      mutationFn: (number: number) =>
        unwrap(api.POST("/api/projects/{key}/branches/{number}/request-review", on(number))),
      onSettled: settled,
    }),
    merge: useMutation({
      mutationFn: (number: number) =>
        unwrap(api.POST("/api/projects/{key}/branches/{number}/merge", on(number))),
      // The merge check builds the merged result first; ask again until it is done.
      retry: (count, error) =>
        error instanceof HttpError && error.code === "pending" && count < 200,
      retryDelay: 3000,
      onSettled: settled,
    }),
    approve: useMutation({
      mutationFn: (number: number) =>
        unwrap(api.POST("/api/projects/{key}/branches/{number}/approve", on(number))),
      onSettled: settled,
    }),
    archive: useMutation({
      mutationFn: (number: number) =>
        unwrap(api.POST("/api/projects/{key}/branches/{number}/archive", on(number))),
      onSettled: settled,
    }),
    reopen: useMutation({
      mutationFn: (number: number) =>
        unwrap(api.POST("/api/projects/{key}/branches/{number}/reopen", on(number))),
      onSettled: settled,
    }),
    duplicate: useMutation({
      mutationFn: (number: number) =>
        unwrap(api.POST("/api/projects/{key}/branches/{number}/duplicate", on(number))),
      onSettled: settled,
    }),
    catchUp: useMutation({
      mutationFn: (number: number) =>
        unwrap(api.POST("/api/projects/{key}/branches/{number}/catch-up", on(number))),
      onSettled: settled,
    }),
    restore: useMutation({
      mutationFn: (number: number) =>
        unwrap(
          api.POST("/api/projects/{key}/versions/{number}/restore", {
            params: { path: { key, number } },
          }),
        ),
      onSettled: settled,
    }),
  };
}

/** Paths changed from `from` to `to` (each `main`, `v<n>`, `b<n>` or a commit). */
export function useDiff(key: string, from: string | undefined, to: string | undefined) {
  return useQuery({
    queryKey: ["diff", key, from, to],
    enabled: !!from && !!to,
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/diff", {
          params: { path: { key }, query: { from: from ?? "", to: to ?? "" } },
        }),
      ),
  });
}

// ---- web agent ---------------------------------------------------------------------------------

/** Whether the web agent is on and which agents it offers. */
export function useAgent(enabled = true) {
  return useQuery({
    queryKey: ["agent"],
    enabled,
    retry: false,
    queryFn: () => unwrap(api.GET("/api/agent")),
  });
}

const running = (runs: AgentRun[] | undefined) =>
  (runs ?? []).some((r) => r.state === "queued" || r.state === "running");

/** A branch's runs (or the project's), oldest first; polls while one is going. */
export function useAgentRuns(key: string, branch: number | undefined, enabled = true) {
  return useQuery({
    queryKey: ["agent-runs", key, branch ?? "all"],
    enabled,
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/agent-runs", {
          params: { path: { key }, query: branch ? { branch } : {} },
        }),
      ),
    refetchInterval: (query) => (running(query.state.data) ? 3000 : false),
  });
}

/** A run's conversation; polls while it runs. */
export function useAgentMessages(key: string, id: number, live: boolean) {
  return useQuery({
    queryKey: ["agent-messages", key, id],
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/agent-runs/{id}/messages", {
          params: { path: { key, id } },
        }),
      ),
    refetchInterval: live ? 2000 : false,
  });
}

export type NewAgentRun = {
  prompt: string;
  context?: unknown;
  branch?: number;
  agent?: AgentKind;
  model?: string;
  images?: { mediaType: "image/png" | "image/jpeg" | "image/gif" | "image/webp"; data: string }[];
};

export function useAgentActions(key: string) {
  const client = useQueryClient();
  const settled = () => {
    void client.invalidateQueries({ queryKey: ["agent-runs", key] });
    void client.invalidateQueries({ queryKey: ["agent-messages", key] });
    void client.invalidateQueries({ queryKey: ["branches", key] });
  };
  return {
    start: useMutation({
      mutationFn: (body: NewAgentRun) =>
        unwrap(api.POST("/api/projects/{key}/agent-runs", { ...path(key), body })),
      onSettled: settled,
    }),
    cancel: useMutation({
      mutationFn: (id: number) =>
        unwrap(
          api.POST("/api/projects/{key}/agent-runs/{id}/cancel", {
            params: { path: { key, id } },
          }),
        ),
      onSettled: settled,
    }),
  };
}

export function useAgentSettings(enabled: boolean) {
  const client = useQueryClient();
  return {
    status: useQuery({
      queryKey: ["admin", "agent"],
      enabled,
      queryFn: () => unwrap(api.GET("/api/admin/agent")),
    }),
    save: useMutation({
      mutationFn: (body: {
        default?: AgentKind;
        models?: Record<string, string>;
        keys?: Record<string, string>;
      }) => unwrap(api.PUT("/api/admin/agent", { body })),
      onSuccess: (status) => {
        client.setQueryData(["admin", "agent"], status);
        void client.invalidateQueries({ queryKey: ["agent"] });
      },
    }),
  };
}

export function useAgentCosts(enabled: boolean) {
  return useQuery({
    queryKey: ["admin", "costs"],
    enabled,
    queryFn: () => unwrap(api.GET("/api/admin/costs")),
  });
}

/** The providers the web agent can run on, each with its API key and sign-in (admins). */
export function useProviders(enabled = true) {
  return useQuery({
    queryKey: ["admin", "providers"],
    enabled,
    queryFn: () => unwrap(api.GET("/api/admin/agent/providers")),
  });
}

const provider = (id: ProviderId) => ({ params: { path: { id } } });

/** Asks a provider whether an API key works; nothing is saved. */
export function useCheckKey() {
  return useMutation({
    mutationFn: (input: { provider: ProviderId; key: string }) =>
      unwrap(
        api.POST("/api/admin/agent/providers/{id}/key/check", {
          ...provider(input.provider),
          body: { key: input.key },
        }),
      ),
  });
}

/** Saving, removing and choosing what the web agent uses for a provider. */
export function useProviderActions() {
  const client = useQueryClient();
  const settled = () => {
    void client.invalidateQueries({ queryKey: ["admin"] });
    void client.invalidateQueries({ queryKey: ["agent"] });
  };
  return {
    saveKey: useMutation({
      mutationFn: (input: { provider: ProviderId; key: string }) =>
        unwrap(
          api.PUT("/api/admin/agent/providers/{id}/key", {
            ...provider(input.provider),
            body: { key: input.key },
          }),
        ),
      onSettled: settled,
    }),
    removeKey: useMutation({
      mutationFn: (id: ProviderId) =>
        unwrapEmpty(api.DELETE("/api/admin/agent/providers/{id}/key", provider(id))),
      onSettled: settled,
    }),
    choose: useMutation({
      mutationFn: (input: { provider: ProviderId; use: "key" | "oauth" }) =>
        unwrap(
          api.PUT("/api/admin/agent/providers/{id}/use", {
            ...provider(input.provider),
            body: { use: input.use },
          }),
        ),
      onSettled: settled,
    }),
    startSignIn: useMutation({
      mutationFn: (id: ProviderId) =>
        unwrap(api.POST("/api/admin/agent/providers/{id}/oauth", provider(id))),
      onSettled: settled,
    }),
    /** Claude: the code its page showed, pasted back. */
    finishSignIn: useMutation({
      mutationFn: (input: { provider: ProviderId; code: string }) =>
        unwrap(
          api.POST("/api/admin/agent/providers/{id}/oauth/code", {
            ...provider(input.provider),
            body: { code: input.code },
          }),
        ),
      onSettled: settled,
    }),
    /** Gives up the sign-in that waits (or dismisses a failed one); a signed-in account stays. */
    cancelSignIn: useMutation({
      mutationFn: (id: ProviderId) =>
        unwrapEmpty(api.DELETE("/api/admin/agent/providers/{id}/oauth", provider(id))),
      onSettled: settled,
    }),
    signOut: useMutation({
      mutationFn: (id: ProviderId) =>
        unwrapEmpty(api.POST("/api/admin/agent/providers/{id}/oauth/sign-out", provider(id))),
      onSettled: settled,
    }),
  };
}

export type Look = { image: string; elements: Element[]; issues: string[] };

/**
 * A screen of a revision as a picture with its element map, rendered on demand (nothing stored):
 * how a draft looks while it is hand-edited. Waits while the revision builds.
 */
export function useLook(
  key: string,
  rev: string | undefined,
  target: string,
  theme: Theme,
  enabled = true,
) {
  return useQuery({
    queryKey: ["look", key, rev, target, theme],
    enabled: enabled && !!rev,
    staleTime: Number.POSITIVE_INFINITY,
    retry: (count, error) => error instanceof HttpError && error.code === "pending" && count < 120,
    retryDelay: 2000,
    queryFn: async (): Promise<Look> => {
      const data = await unwrap(
        api.POST("/api/projects/{key}/check", {
          ...path(key),
          body: { ...(rev ? { rev } : {}), targets: [target], themes: [theme], image: true },
        }),
      );
      const result = data.results[0];
      return {
        image: result?.image ? `data:image/jpeg;base64,${result.image}` : "",
        elements: result?.elements ?? [],
        issues: result?.issues ?? [],
      };
    },
  });
}

/** What a hand edit can change on an element (`src` / `use` as the element map has them). */
export function useDesign(
  key: string,
  rev: string | undefined,
  src: string | undefined,
  use: string | undefined,
) {
  return useQuery({
    queryKey: ["design", key, rev, src, use],
    enabled: !!src || !!use,
    queryFn: () =>
      unwrap(
        api.GET("/api/projects/{key}/design", {
          params: {
            path: { key },
            query: { ...(rev ? { rev } : {}), ...(src ? { src } : {}), ...(use ? { use } : {}) },
          },
        }),
      ),
  });
}

export type HandEditInput = Schemas["HandEditInput"];

/** Hand edits: apply one, check it (asking again while the draft builds), undo / redo. */
export function useDesignActions(key: string) {
  const client = useQueryClient();
  const settled = () => {
    void client.invalidateQueries({ queryKey: ["branches", key] });
    void client.invalidateQueries({ queryKey: ["branch", key] });
    void client.invalidateQueries({ queryKey: ["design", key] });
  };
  return {
    edit: useMutation({
      mutationFn: (body: HandEditInput) =>
        unwrap(api.POST("/api/projects/{key}/design/edits", { ...path(key), body })),
      onSettled: settled,
    }),
    check: useMutation({
      mutationFn: (body: { branch: number; path: string; before: string; commit: string }) =>
        unwrap(api.POST("/api/projects/{key}/design/check", { ...path(key), body })),
      retry: (count, error) =>
        error instanceof HttpError && error.code === "pending" && count < 120,
      retryDelay: 2000,
      onSettled: settled,
    }),
    restore: useMutation({
      mutationFn: (body: { branch: number; path: string; to: string; expect: string }) =>
        unwrap(api.POST("/api/projects/{key}/design/restore", { ...path(key), body })),
      onSettled: settled,
    }),
  };
}
