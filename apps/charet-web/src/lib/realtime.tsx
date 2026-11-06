// The realtime channel of a project room: who is here (and what they point at), what changed.
// Events refresh the queries they affect, so comments, versions and captures appear for
// everyone without a reload.

import { useQueryClient } from "@tanstack/react-query";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useNavigate } from "react-router";
import { useVisitorName } from "./visitor.ts";

export interface Peer {
  id: string;
  person: string;
  name: string;
  kind: "local" | "user" | "visitor";
  role: string;
}

/** What someone shares about where they are; every field is optional. */
export interface Presence {
  /** The route they look at (`/p/web/screens`). */
  view?: string;
  /** Pointer in the coordinates of the view's surface. */
  cursor?: { x: number; y: number } | null;
  /** The element they picked on a screen, in the capture's pixels. */
  selection?: { x: number; y: number; w: number; h: number } | null;
  /** Pan and zoom of a board view. */
  viewport?: { x: number; y: number; zoom: number } | null;
  /** A short message shown next to their cursor. */
  chat?: string | null;
  /** Asks everyone to follow them. */
  spotlight?: boolean;
}

export interface Working {
  person: string;
  name: string;
}

interface RoomState {
  connected: boolean;
  me?: Peer;
  /** By connection; `at` is when it last moved, so a person's latest tab wins. */
  peers: Map<string, Entry>;
  working: Working[];
}

interface Room extends RoomState {
  /** Shares (merges into) this connection's presence. */
  share: (presence: Presence) => void;
  /** The person whose view this tab follows (by choice or their spotlight). */
  following: string | null;
  follow: (person: string | null) => void;
}

const RoomContext = createContext<Room>({
  connected: false,
  peers: new Map(),
  working: [],
  share: () => {},
  following: null,
  follow: () => {},
});

export function useRoom(): Room {
  return useContext(RoomContext);
}

/** People, not connections: someone with two tabs shows once. */
type Entry = { peer: Peer; presence: Presence; at: number };

/** Everyone else here, one entry per person: their most recently active tab. */
export function usePeople(): Entry[] {
  const { peers, me } = useRoom();
  const seen = new Map<string, Entry>();
  for (const entry of peers.values()) {
    if (entry.peer.person === me?.person) continue;
    const known = seen.get(entry.peer.person);
    if (!known || entry.at >= known.at) seen.set(entry.peer.person, entry);
  }
  return [...seen.values()];
}

/** A stable color per person. */
export function colorOf(person: string): string {
  let hash = 0;
  for (const char of person) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return `oklch(0.62 0.17 ${hash % 360})`;
}

type Message =
  | {
      type: "hello";
      you: Peer;
      seq: number;
      peers: { peer: Peer; presence: Presence | null }[];
      missed: Message[];
      reload: boolean;
      working: Working[];
    }
  | { type: "join"; peer: Peer }
  | { type: "leave"; id: string }
  | { type: "presence"; from: string; data: Presence }
  | { type: "event"; seq: number; kind: string; data: unknown }
  | { type: "inbox" }
  | { type: "admin"; kind: string; data: unknown }
  | { type: "working"; working: Working[] };

export function RealtimeProvider({ project, children }: { project: string; children: ReactNode }) {
  const client = useQueryClient();
  const navigate = useNavigate();
  const [state, setState] = useState<RoomState>({
    connected: false,
    peers: new Map(),
    working: [],
  });
  const [chosen, setChosen] = useState<string | null>(null);
  // Presenters someone stepped away from; their spotlight no longer pulls this tab.
  const [declined, setDeclined] = useState<Set<string>>(new Set());
  const socket = useRef<WebSocket | undefined>(undefined);
  const mine = useRef<Presence>({});

  const send = useCallback((presence: Presence) => {
    const ws = socket.current;
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: "presence", data: presence }));
    }
  }, []);

  const refresh = useCallback(
    (kind: string) => {
      const keys: unknown[][] = [];
      if (kind.startsWith("comment.")) keys.push(["comments", project]);
      if (kind === "branch.updated") keys.push(["branches", project], ["branch", project]);
      if (kind === "version.handoff") keys.push(["versions", project]);
      if (kind === "version.created")
        keys.push(
          ["versions", project],
          ["branches", project],
          ["manifest", project],
          ["captures", project],
          ["components", project],
          ["preview", project],
          ["files", project],
          ["file", project],
        );
      if (kind === "capture.updated")
        keys.push(
          ["captures", project],
          ["components", project],
          ["elements", project],
          ["comments", project],
        );
      if (kind === "project.updated") keys.push(["project", project], ["projects"], ["me"]);
      if (kind === "agent.updated")
        keys.push(["agent-runs", project], ["agent-messages", project], ["branches", project]);
      // A first load still in flight would answer the refetch too (no data yet means no restart),
      // possibly without what this event announced: cancel it, then fetch again.
      for (const queryKey of keys)
        void client.cancelQueries({ queryKey }).then(() => client.invalidateQueries({ queryKey }));
      if (kind === "project.deleted") navigate("/");
    },
    [client, navigate, project],
  );

  useEffect(() => {
    let seq: number | undefined;
    let stopped = false;
    let retry = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const connect = () => {
      const scheme = window.location.protocol === "https:" ? "wss" : "ws";
      const since = seq === undefined ? "" : `&since=${seq}`;
      const ws = new WebSocket(
        `${scheme}://${window.location.host}/api/ws?project=${encodeURIComponent(project)}${since}`,
      );
      socket.current = ws;
      ws.onopen = () => {
        retry = 0;
      };
      ws.onmessage = (event) => {
        const message = JSON.parse(String(event.data)) as Message;
        handle(message);
      };
      ws.onclose = () => {
        socket.current = undefined;
        setState((s) => ({ ...s, connected: false, peers: new Map() }));
        if (stopped) return;
        // Maybe access was taken away: the auth gate reacts to a failing /me.
        void client.invalidateQueries({ queryKey: ["me"] });
        retry += 1;
        timer = setTimeout(connect, Math.min(30_000, 500 * 2 ** retry));
      };
    };
    const handle = (message: Message) => {
      switch (message.type) {
        case "hello": {
          if (message.reload) {
            void client.invalidateQueries();
          } else if (seq === undefined) {
            // First connection of this page: what loaded before the socket opened may already
            // be behind (a comment made in between). Comments and captures are cheap to refetch.
            refresh("capture.updated");
          }
          for (const missed of message.missed) handle(missed);
          seq = message.seq;
          const peers = new Map<string, Entry>();
          for (const { peer, presence } of message.peers)
            peers.set(peer.id, { peer, presence: presence ?? {}, at: 0 });
          setState({ connected: true, me: message.you, peers, working: message.working });
          // Tell the others where this tab is right away.
          if (Object.keys(mine.current).length > 0) send(mine.current);
          break;
        }
        case "join":
          setState((s) => {
            const peers = new Map(s.peers);
            peers.set(message.peer.id, { peer: message.peer, presence: {}, at: Date.now() });
            return { ...s, peers };
          });
          // A newcomer learns where everyone is.
          if (Object.keys(mine.current).length > 0) send(mine.current);
          break;
        case "leave":
          setState((s) => {
            const peers = new Map(s.peers);
            peers.delete(message.id);
            return { ...s, peers };
          });
          break;
        case "presence":
          setState((s) => {
            const entry = s.peers.get(message.from);
            if (!entry) return s;
            const peers = new Map(s.peers);
            peers.set(message.from, { peer: entry.peer, presence: message.data, at: Date.now() });
            return { ...s, peers };
          });
          break;
        case "event":
          seq = message.seq;
          refresh(message.kind);
          break;
        case "admin":
          void client.invalidateQueries({ queryKey: ["admin"] });
          break;
        case "inbox":
          void client.invalidateQueries({ queryKey: ["notifications"] });
          break;
        case "working":
          setState((s) => ({ ...s, working: message.working }));
          break;
      }
    };
    connect();
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
      socket.current?.close();
    };
  }, [client, project, refresh, send]);

  const share = useCallback(
    (presence: Presence) => {
      mine.current = { ...mine.current, ...presence };
      send(mine.current);
    },
    [send],
  );

  // Following = the person picked, else whoever has the spotlight (unless stepped away from).
  const presenter = [...state.peers.values()].find(
    ({ peer, presence }) =>
      presence.spotlight && peer.person !== state.me?.person && !declined.has(peer.person),
  )?.peer.person;
  const following = chosen ?? presenter ?? null;
  const follow = useCallback(
    (person: string | null) => {
      if (person === null && following) setDeclined((d) => new Set(d).add(following));
      if (person)
        setDeclined((d) => {
          const next = new Set(d);
          next.delete(person);
          return next;
        });
      setChosen(person);
    },
    [following],
  );

  // Visitors show under their made-up name in this page's language.
  const visitorName = useVisitorName();
  const value = useMemo(() => {
    const named = (peer: Peer): Peer =>
      peer.kind === "visitor"
        ? { ...peer, name: visitorName(peer.person.replace(/^visitor:/, "")) }
        : peer;
    const peers = new Map(
      [...state.peers].map(([id, entry]) => [id, { ...entry, peer: named(entry.peer) }]),
    );
    return {
      ...state,
      peers,
      ...(state.me ? { me: named(state.me) } : {}),
      share,
      following,
      follow,
    };
  }, [state, share, following, follow, visitorName]);
  return <RoomContext value={value}>{children}</RoomContext>;
}

/** The latest presence of the person this tab follows. */
export function useFollowed(): Entry | undefined {
  const { following } = useRoom();
  const people = usePeople();
  if (!following) return undefined;
  return people.find((entry) => entry.peer.person === following);
}

/** Calls `fn` at most every `ms`; the last call always lands. */
export function throttle<A extends unknown[]>(fn: (...args: A) => void, ms: number) {
  let last = 0;
  let pending: ReturnType<typeof setTimeout> | undefined;
  let latest: A | undefined;
  return (...args: A) => {
    latest = args;
    const wait = last + ms - Date.now();
    if (wait <= 0) {
      last = Date.now();
      fn(...args);
    } else if (!pending) {
      pending = setTimeout(() => {
        pending = undefined;
        last = Date.now();
        if (latest) fn(...latest);
      }, wait);
    }
  };
}
