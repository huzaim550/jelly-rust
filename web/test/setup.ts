import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";

/**
 * Install a `localStorage` if the environment lacks one.
 *
 * jsdom does not always expose it as a bare global under Node 26, and the app
 * reads it during module import. Defining it up front keeps the failure here
 * rather than surfacing as an app-level crash.
 */
if (typeof globalThis.localStorage === "undefined") {
  const map = new Map<string, string>();
  const shim = {
    getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
    setItem: (k: string, v: string) => void map.set(k, String(v)),
    removeItem: (k: string) => void map.delete(k),
    clear: () => map.clear(),
    key: (i: number) => [...map.keys()][i] ?? null,
    get length() {
      return map.size;
    },
  };
  Object.defineProperty(globalThis, "localStorage", {
    value: shim,
    configurable: true,
    writable: true,
  });
}

/** Clear stored session state between tests. */
export function resetStorage(): void {
  globalThis.localStorage.clear();
}

/** Pretend a reload happened with this token already stored. */
export function setStoredToken(token: string): void {
  globalThis.localStorage.setItem("jf.token", token);
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  resetStorage();
});

// jsdom has no media stack, and the Player touches these on mount.
if (!HTMLMediaElement.prototype.play) {
  HTMLMediaElement.prototype.play = () => Promise.resolve();
}
Object.defineProperty(HTMLMediaElement.prototype, "duration", {
  configurable: true,
  get: () => 600,
});

/**
 * Install a fake server.
 *
 * Routes are matched newest-first, so a test can override a seeded route by
 * registering the same path again. Unmatched requests fail loudly rather than
 * resolving to null, so a test that forgets a route reports "unexpected
 * request" instead of quietly rendering an empty screen.
 */
export interface FakeServer {
  /** Register a JSON response for `METHOD /path`. */
  on(method: string, path: string, body: unknown, status?: number): void;
  /** Calls received, for assertions. */
  calls: Array<{ method: string; path: string; query: string; body?: unknown }>;
  restore: () => void;
}

export function mockServer(): FakeServer {
  type Route = { method: string; path: string; body: unknown; status: number };

  const routes: Route[] = [];
  const calls: FakeServer["calls"] = [];

  const original = globalThis.fetch;

  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    const method = (init?.method ?? "GET").toUpperCase();
    const path = url.split("?")[0] ?? url;

    let parsed: unknown;
    if (init?.body) {
      try {
        parsed = JSON.parse(init.body as string);
      } catch {
        parsed = undefined;
      }
    }
    const query = url.includes("?") ? url.slice(url.indexOf("?") + 1) : "";
    calls.push({ method, path, query, body: parsed });

    // Insertion order does not matter: a route is keyed by method+path, so a
    // later registration for the same key replaces the earlier one. This lets
    // a test override a seeded default regardless of setup order.
    const route = routes.find((r) => r.method === method && r.path === path);

    if (!route) {
      return new Response(
        JSON.stringify({ Message: `no fake route for ${method} ${path}`, Status: 404 }),
        { status: 404, headers: { "Content-Type": "application/json" } },
      );
    }

    // 204 must have a null body, and the Response constructor rejects a
    // non-null one. Jellyfin answers its mutating endpoints with 204, so this
    // path matters.
    const noContent = route.status === 204 || route.body === undefined;

    return new Response(noContent ? null : JSON.stringify(route.body), {
      status: route.status,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;

  return {
    on(method, path, body, status = 200) {
      const upper = method.toUpperCase();
      const existing = routes.findIndex((r) => r.method === upper && r.path === path);
      const route = { method: upper, path, body, status };

      if (existing >= 0) routes[existing] = route;
      else routes.push(route);

      return this;
    },
    calls,
    restore() {
      globalThis.fetch = original;
    },
  };
}

/** A signed-in admin, which most screens need. */
export const ADMIN_USER = {
  Id: "user-1",
  Name: "huzaim",
  ServerId: "server-1",
  PrimaryImageTag: null,
  HasPassword: true,
  Policy: {
    EnableAllFolders: true,
    EnabledFolders: [],
    EnableMediaPlayback: true,
    EnableAudioPlaybackTranscoding: true,
    EnableVideoPlaybackTranscoding: true,
    EnablePlaybackRemuxing: true,
    EnableContentDeletion: true,
    EnableContentDownloading: true,
    EnableSyncTranscoding: true,
    EnableMediaStreamEditing: false,
    IsAdministrator: true,
    IsHidden: false,
    IsDisabled: false,
    EnableLiveTvManagement: false,
    EnableLiveTvAccess: false,
    MaxActiveSessions: 0,
    AuthenticationProviderId: "Default",
    PasswordResetProviderId: "Default",
    SyncPlayAccess: "CreateAndJoinGroups",
  },
};

/** Route set for a signed-in admin on a server with one library. */
export function seedAdmin(server: FakeServer, extra: { users?: unknown[] } = {}) {
  server.on("GET", "/System/Info/Public", {
    ServerName: "Test Server",
    Version: "0.1.0",
    Id: "server-1",
    StartupWizardCompleted: true,
  });
  server.on("GET", "/Users/Me", ADMIN_USER);
  server.on("GET", "/Library/VirtualFolders", [
    {
      Name: "Films",
      Locations: ["/media/movies"],
      CollectionType: "Movies",
      ItemId: "lib-1",
      RefreshProgress: null,
      RefreshStatus: null,
    },
  ]);
  server.on("GET", "/Library/ScanStatus", { scanning: false, libraries: {} });
  server.on("GET", "/Sessions", []);
  server.on("POST", "/System/Restart", undefined, 202);
  server.on("POST", "/System/Shutdown", undefined, 202);
  server.on("GET", "/Users", extra.users ?? [ADMIN_USER]);
  server.on("GET", "/Users/Public", {
    ServerId: "server-1",
    ServerName: "Test Server",
    Users: extra.users ?? [ADMIN_USER],
  });
  server.on("GET", `/Users/${ADMIN_USER.Id}/Views`, {
    Items: [
      {
        Id: "root-1",
        Name: "Films",
        Type: "UserView",
        ServerId: "server-1",
        IsFolder: true,
        CollectionType: "Movies",
      },
    ],
    TotalRecordCount: 1,
    StartIndex: 0,
  });
  server.on("GET", `/Users/${ADMIN_USER.Id}/Items/Resume`, {
    Items: [],
    TotalRecordCount: 0,
    StartIndex: 0,
  });
  server.on("GET", `/Users/${ADMIN_USER.Id}/Items`, {
    Items: [],
    TotalRecordCount: 0,
    StartIndex: 0,
  });
  server.on("GET", `/Users/${ADMIN_USER.Id}/Shows/NextUp`, {
    Items: [],
    TotalRecordCount: 0,
    StartIndex: 0,
  });
  return server;
}