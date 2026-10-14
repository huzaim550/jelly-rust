/**
 * API client.
 *
 * Talks to the Jellyfin-compatible routes the Rust server exposes. The token
 * is held in module scope and mirrored to localStorage so a reload does not
 * bounce the user back to the login screen.
 */

import type {
  AuthenticationResult,
  BaseItem,
  PlaybackInfo,
  PublicSystemInfo,
  PublicUsers,
  QueryResult,
} from "../types";

const TOKEN_KEY = "jf.token";
const DEVICE_KEY = "jf.device";

/**
 * One directory from `/Library/PhysicalPaths`.
 *
 * PascalCase because that is what the endpoint sends: this API mirrors
 * Jellyfin's wire format, where every field is capitalised. Reading `name`
 * here silently yields `undefined` and the picker renders "undefined/".
 */
export interface PhysicalPath {
  Name: string;
  Path: string;
}

export interface SessionSummary {
  Id: string;
  UserName: string;
  Client: string;
  DeviceName: string;
  ApplicationVersion: string;
  IsActive: boolean;
}

/**
 * `localStorage`, but never throwing.
 *
 * Firefox private browsing and Safari's restricted mode throw on *accessing*
 * the object at all, not just on writing. Since this module reads storage at
 * import time, an unguarded access white-screens the whole app in those
 * browsers. Falling back to an in-memory map keeps the session alive for the
 * page's lifetime, which is the best that can be done without persistence.
 */
const memoryStore = new Map<string, string>();

const store: {
  get(key: string): string | null;
  set(key: string, value: string): void;
  remove(key: string): void;
} = (() => {
  try {
    const probe = "__jf_probe__";
    window.localStorage.setItem(probe, "1");
    window.localStorage.removeItem(probe);

    return {
      get: (key) => window.localStorage.getItem(key),
      set: (key, value) => window.localStorage.setItem(key, value),
      remove: (key) => window.localStorage.removeItem(key),
    };
  } catch {
    return {
      get: (key) => memoryStore.get(key) ?? null,
      set: (key, value) => void memoryStore.set(key, value),
      remove: (key) => void memoryStore.delete(key),
    };
  }
})();

let token: string | null = store.get(TOKEN_KEY);

/** Why a request failed, with enough detail to show something useful. */
export class ApiError extends Error {
  readonly status: number;
  readonly path: string;

  constructor(status: number, path: string, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.path = path;
  }

  /** True when the session is gone and the user must sign in again. */
  get isAuthFailure(): boolean {
    return this.status === 401;
  }
}

function setToken(next: string | null): void {
  token = next;
  if (next) store.set(TOKEN_KEY, next);
  else store.remove(TOKEN_KEY);
}

function buildQuery(params: Record<string, unknown>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === "") continue;
    search.set(key, String(value));
  }
  const query = search.toString();
  return query ? `?${query}` : "";
}

async function request<T>(
  method: string,
  path: string,
  options: { body?: unknown; params?: Record<string, unknown>; auth?: boolean } = {},
): Promise<T> {
  const { body, params, auth = true } = options;

  const headers: Record<string, string> = {
    Accept: "application/json",
    "X-Emby-Client": "jellyfin-clone-web",
    "X-Emby-Device-Name": "Web Browser",
    "X-Emby-Device-Id": deviceId(),
  };
  if (body !== undefined) headers["Content-Type"] = "application/json";
  // Public endpoints must not carry a stale token; the server would reject it
  // on a half-initialised session.
  if (auth && token) headers["X-Emby-Token"] = token;

  const response = await fetch(`${path}${buildQuery(params ?? {})}`, {
    method,
    headers,
    credentials: "same-origin",
    body: body === undefined ? undefined : JSON.stringify(body),
  });

  if (response.status === 204) return undefined as T;

  const text = await response.text();
  let parsed: unknown = undefined;
  if (text) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = undefined;
    }
  }

  if (!response.ok) {
    const detail =
      (parsed as { Message?: string } | undefined)?.Message ??
      text.slice(0, 200) ??
      response.statusText;
    throw new ApiError(response.status, path, detail || "Request failed");
  }

  return parsed as T;
}

/** A stable per-browser device id, so sessions are distinguishable. */
function deviceId(): string {
  let id = store.get(DEVICE_KEY);
  if (!id) {
    // `crypto.randomUUID` needs a secure context, which a LAN-served http://
    // origin is not. Fall back so the client still works on a home network.
    id =
      typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : `jf-${Math.random().toString(36).slice(2)}-${Date.now().toString(36)}`;
    store.set(DEVICE_KEY, id);
  }
  return id;
}

export const api = {
  get currentToken(): string | null {
    return token;
  },

  // ------------------------------------------------------------------ system

  publicInfo(): Promise<PublicSystemInfo> {
    return request("GET", "/System/Info/Public", { auth: false });
  },

  publicUsers(): Promise<PublicUsers> {
    return request("GET", "/Users/Public", { auth: false });
  },

  // -------------------------------------------------------------------- auth

  async login(username: string, password: string): Promise<AuthenticationResult> {
    const result = await request<AuthenticationResult>(
      "POST",
      "/Users/AuthenticateByName",
      { body: { Username: username, Pw: password }, auth: false },
    );
    setToken(result.AccessToken);
    return result;
  },

  async logout(): Promise<void> {
    try {
      if (token) await request("DELETE", "/Sessions/Logout");
    } catch {
      // Signing out locally must succeed even if the server is unreachable;
      // trapping the user in a signed-in state is worse than a stale session.
    }
    setToken(null);
  },

  /**
   * Re-adopt a stored token. Returns false if the server rejects it.
   *
   * Re-reads storage rather than trusting the module-level `token`, which was
   * captured when this module was first imported. A token written afterwards —
   * by a sign-in in another tab, or by a test priming storage — would otherwise
   * be invisible and the user would be silently signed out.
   */
  async restore(): Promise<boolean> {
    setToken(store.get(TOKEN_KEY));
    if (!token) return false;

    try {
      await request("GET", "/Users/Me");
      return true;
    } catch {
      setToken(null);
      return false;
    }
  },

  me(): Promise<import("../types").User> {
    return request("GET", "/Users/Me");
  },

  // ----------------------------------------------------------------- library

  async views(userId: string): Promise<BaseItem[]> {
    const result = await request<QueryResult<BaseItem>>("GET", `/Users/${userId}/Views`);
    return result.Items;
  },

  async items(
    userId: string,
    options: {
      parentId?: string;
      includeItemTypes?: string;
      recursive?: boolean;
      searchTerm?: string;
      sortBy?: string;
      limit?: number;
      startIndex?: number;
      fields?: string;
    } = {},
  ): Promise<QueryResult<BaseItem>> {
    const { parentId, ...params } = options;
    const path = parentId
      ? `/Users/${userId}/Items/${parentId}`
      : `/Users/${userId}/Items`;
    return request("GET", path, { params });
  },

  resume(userId: string, limit = 20): Promise<QueryResult<BaseItem>> {
    return request("GET", `/Users/${userId}/Items/Resume`, {
      params: {
        Limit: limit,
        Recursive: "true",
        Fields: "Overview,Genres,People,ProductionYear,RunTimeTicks,UserData",
      },
    });
  },

  /**
   * Most recently added items.
   *
   * A sorted items query rather than a dedicated endpoint: the server already
   * supports `DateCreated` ordering, and a new route would be a second thing to
   * keep in step with the item query's filtering.
   */
  latest(userId: string, limit = 20): Promise<QueryResult<BaseItem>> {
    return request("GET", `/Users/${userId}/Items`, {
      params: {
        Limit: limit,
        Recursive: "true",
        SortBy: "DateCreated",
        SortOrder: "Descending",
        Fields: "Overview,Genres,People,ProductionYear,RunTimeTicks,UserData,DateCreated",
      },
    });
  },

  nextUp(userId: string, limit = 20): Promise<QueryResult<BaseItem>> {
    return request("GET", `/Users/${userId}/Shows/NextUp`, {
      params: { Limit: limit, Fields: "Overview,ProductionYear,RunTimeTicks,UserData" },
    });
  },

  item(userId: string, itemId: string): Promise<BaseItem> {
    return request("GET", `/Users/${userId}/Items/${itemId}`);
  },

  seasons(seriesId: string): Promise<QueryResult<BaseItem>> {
    return request("GET", `/Shows/${seriesId}/Seasons`);
  },

  episodes(seriesId: string, seasonId: string): Promise<QueryResult<BaseItem>> {
    return request("GET", `/Shows/${seriesId}/Episodes`, {
      params: { SeasonId: seasonId, Fields: "Overview,RunTimeTicks,ProductionYear" },
    });
  },

  // ---------------------------------------------------------------- playback

  playbackInfo(itemId: string, userId: string): Promise<PlaybackInfo> {
    return request("GET", `/Items/${itemId}/PlaybackInfo`, {
      params: { UserId: userId },
    });
  },

  /** Direct-stream URL. `<video>` cannot set headers, so the token is a query
   * parameter here — the server accepts it on media and image routes. */
  streamUrl(itemId: string): string {
    return `/Videos/${itemId}/stream?static=true&api_key=${token ?? ""}`;
  },

  reportPlaybackStart(itemId: string, positionTicks: number, playSessionId: string): Promise<void> {
    return request("POST", "/Sessions/Playing", {
      body: {
        ItemId: itemId,
        PositionTicks: positionTicks,
        CanSeek: true,
        PlaySessionId: playSessionId,
      },
      params: { DeviceId: deviceId(), DeviceName: "Web Browser" },
    });
  },

  reportProgress(itemId: string, positionTicks: number, paused: boolean): Promise<void> {
    return request("POST", `/UserPlayedItems/${itemId}/Progress`, {
      body: { PositionTicks: positionTicks, IsPaused: paused, CanSeek: true },
    });
  },

  reportStopped(itemId: string, positionTicks: number): Promise<void> {
    return request("POST", "/Sessions/Playing/Stopped", {
      body: { ItemId: itemId, PositionTicks: positionTicks },
    });
  },

  setFavorite(itemId: string, favorite: boolean): Promise<void> {
    return request("POST", `/UserFavoriteItems/${itemId}`, { params: { favorite } });
  },

  // ----------------------------------------------------- library management

  virtualFolders(): Promise<import("../library").VirtualFolder[]> {
    return request("GET", "/Library/VirtualFolders");
  },

  addVirtualFolder(name: string, collectionType: string, path: string): Promise<void> {
    return request("POST", "/Library/VirtualFolders", {
      params: { name, collectionType, paths: path, refreshLibrary: "true" },
    });
  },

  deleteVirtualFolder(name: string): Promise<void> {
    return request("DELETE", "/Library/VirtualFolders", { params: { name } });
  },

  /** Start a scan of every library. Returns immediately; progress arrives over SSE. */
  refreshLibrary(): Promise<void> {
    return request("POST", "/Library/Refresh");
  },

  scanStatus(): Promise<{
    scanning: boolean;
    libraries: Record<string, { state: string; progress: number | null; processed: number; detail: string | null }>;
  }> {
    return request("GET", "/Library/ScanStatus");
  },

  sessions(): Promise<SessionSummary[]> {
    return request("GET", "/Sessions");
  },

  serverControl(action: "restart" | "shutdown"): Promise<void> {
    const path = action === "restart" ? "/System/Restart" : "/System/Shutdown";
    return request("POST", path);
  },

  /** List a directory on the server, for the folder picker. */
  physicalPaths(path = "/"): Promise<PhysicalPath[]> {
    return request("GET", "/Library/PhysicalPaths", { params: { path } });
  },

  plugins(): Promise<{
    ApiVersion: string;
    Loaded: string[];
    Count: number;
    Report?: {
      loaded: Array<{ id: string; name: string; version: string; metadataProviders: string[] }>;
      failed: Array<{ id: string; reason: string }>;
      apiVersion: string;
    };
  }> {
    return request("GET", "/Plugins");
  },

  omdbApiKeyStatus(): Promise<{ Configured: boolean }> {
    return request("GET", "/Plugins/omdb/Configuration");
  },

  saveOmdbApiKey(apiKey: string): Promise<{ Configured: boolean }> {
    return request("POST", "/Plugins/omdb/Configuration", { body: { ApiKey: apiKey } });
  },

  removeOmdbApiKey(): Promise<{ Configured: boolean }> {
    return request("DELETE", "/Plugins/omdb/Configuration");
  },

  // ------------------------------------------------------------- onboarding

  startupConfiguration(): Promise<{
    ServerName: string | null;
    UICulture: string | null;
    PreferredMetadataLanguage: string | null;
    MetadataCountryCode: string | null;
  }> {
    return request("GET", "/Startup/Configuration", { auth: false });
  },

  postStartupConfiguration(config: {
    ServerName?: string;
    UICulture?: string;
    PreferredMetadataLanguage?: string;
    MetadataCountryCode?: string;
  }): Promise<void> {
    return request("POST", "/Startup/Configuration", { body: config, auth: false });
  },

  /** Null when no account exists yet, which is what drives the wizard. */
  firstUser(): Promise<{ Name: string; HasPassword: boolean } | null> {
    return request("GET", "/Startup/FirstUser", { auth: false });
  },

  /** Create the administrator. Only valid during onboarding. */
  createStartupUser(name: string, password?: string): Promise<void> {
    return request("POST", "/Startup/User", {
      body: { Name: name, Password: password },
      auth: false,
    });
  },

  completeStartup(): Promise<void> {
    return request("POST", "/Startup/Complete", { body: {}, auth: false });
  },

  // ----------------------------------------------------------------- helpers

  imageUrl(itemId: string, kind: string, width?: number): string {
    const params = new URLSearchParams();
    if (width) params.set("maxWidth", String(width));
    if (token) params.set("api_key", token);
    const query = params.toString();
    return `/Items/${itemId}/Images/${kind}${query ? `?${query}` : ""}`;
  },
};