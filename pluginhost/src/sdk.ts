/**
 * The plugin SDK.
 *
 * A plugin is a module exporting any of the optional hooks below. The host
 * imports it and calls them; the SDK gives it a `host` and `config` handle for
 * the two capabilities every plugin needs.
 *
 * ```ts
 * import { host, config, definePlugin, MetadataResult } from "../sdk.js";
 *
 * export default definePlugin({
 *   id: "example",
 *   async fetchMetadata(query) {
 *     if (!query.title) return undefined;
 *     host.log("info", `resolving ${query.title}`);
 *     return { title: query.title } satisfies MetadataResult;
 *   },
 * });
 * ```
 *
 * Hook contracts that matter:
 *
 * - `fetchMetadata` returns `undefined` to mean "I have nothing". The server
 *   then tries the next provider in the chain; returning an empty object would
 *   count as a result and stop the chain.
 * - Hooks are deadline-bounded by the host. A plugin that hangs is timed out
 *   and its reply discarded — the process survives, but the call is lost.
 *   Prefer `fetch` with an `AbortSignal` over an unbounded call.
 * - Values cross the process boundary as JSON, so `undefined` fields are
 *   dropped rather than sent as `null`.
 */

/** Host API version this SDK targets. */
export const API_VERSION = "1.0";

/** Capabilities a plugin may request in its manifest. */
export type Capability =
  | "readItems"
  | "network"
  | "httpRoutes"
  | "config"
  | "events"
  | "writeMetadata";

export type PersonKind = "actor" | "director" | "writer" | "producer" | "guestStar";

export type ImageKind = "primary" | "backdrop" | "logo" | "banner" | "thumb";

/** One contributor attached to an item. */
export interface MetadataPerson {
  name: string;
  role?: string;
  kind: PersonKind;
}

/**
 * An artwork reference.
 *
 * `path` may be an absolute URL the host will download, or a filename relative
 * to the media file for sidecar art.
 */
export interface MetadataImage {
  path: string;
  kind?: ImageKind;
  index?: number;
  width?: number;
  height?: number;
}

/** What a metadata provider returns. Every field is optional. */
export interface MetadataResult {
  title?: string;
  originalTitle?: string;
  sortTitle?: string;
  overview?: string;
  tagline?: string;
  year?: number;
  premiereDate?: string;
  communityRating?: number;
  criticRating?: number;
  officialRating?: string;
  status?: string;
  genres?: string[];
  studios?: string[];
  /** Runtime in 100-nanosecond ticks, matching Jellyfin's convention. */
  runTimeTicks?: number;
  people?: MetadataPerson[];
  images?: MetadataImage[];
  /** Provider ids to persist, keyed by provider name. */
  externalIds?: Record<string, string>;
}

/** What the host asks a provider to resolve. */
export interface MetadataQuery {
  title: string;
  year?: number;
  kind: string;
  /** Provider ids already known, from an NFO or another provider. */
  knownIds: Record<string, string>;
  /** Languages to prefer, most preferred first. */
  preferredLanguages: string[];
}

/** The narrow item shape plugins see. */
export interface ItemSummary {
  id: string;
  name: string;
  kind: string;
  year?: number;
  path: string;
}

/** Events delivered to `onEvent`. */
export type PluginEvent =
  | { type: "scanComplete"; libraryId: string }
  | { type: "itemDiscovered"; itemId: string; libraryId: string }
  | { type: "playbackStart"; itemId: string; userId: string }
  | { type: "playbackStop"; itemId: string; userId: string; positionTicks: number }
  | { type: "userAuthenticated"; userId: string };

/** The hooks a plugin may implement. All are optional. */
export interface PluginHooks {
  /** Called once after the module is imported. Must be fast. */
  setup?: () => void | Promise<void>;

  /** Resolve metadata for one item. Return `undefined` to decline. */
  fetchMetadata?: (query: MetadataQuery) => MetadataResult | undefined | Promise<MetadataResult | undefined>;

  /** React to a host event. Must return fast; errors are logged, not fatal. */
  onEvent?: (event: PluginEvent) => void | Promise<void>;

  /** Called at shutdown. Release anything held here. */
  teardown?: () => void | Promise<void>;
}

// ---------------------------------------------------------------------------
// Host interface
// ---------------------------------------------------------------------------

/**
 * Internal bridge to the server process.
 *
 * Replaced during `install()`. Plugins never construct this; they import the
 * singleton `host` below.
 */
interface HostBridge {
  log(level: string, message: string): void;
  getItem(itemId: string): Promise<ItemSummary | undefined>;
  apiVersion(): string;
  pluginId(): string;
  configGet(key: string): Promise<string | undefined>;
  configSet(key: string, value: string): Promise<void>;
  configAll(): Promise<Record<string, string>>;
}

/**
 * Server capabilities exposed to plugins.
 *
 * Every method is a request across the process boundary, so none of them may
 * be called in a hot loop: a metadata provider making 50 `getItem` calls pays
 * 50 round trips.
 */
export const host: HostBridge & {
  /** Capabilities this plugin's manifest granted. Replaced during `install`. */
  capabilities: Capability[];
} = (() => {
  const bridge = {} as HostBridge & { capabilities: Capability[] };

  // Fail loudly rather than silently no-op: a plugin logging through an
  // uninstalled bridge is a packaging bug, and a silent one is very hard to
  // find from the plugin side.
  const uninstalled = (): never => {
    throw new Error(
      "the jellyfin-clone plugin SDK is not installed; " +
        "import this module through the plugin host rather than directly",
    );
  };

  bridge.log = () => uninstalled();
  bridge.getItem = async () => uninstalled();
  bridge.apiVersion = () => uninstalled();
  bridge.pluginId = () => uninstalled();
  bridge.configGet = async () => uninstalled();
  bridge.configSet = async () => uninstalled();
  bridge.configAll = async () => uninstalled();
  bridge.capabilities = [];

  return bridge;
})();

/** Settings namespaced to this plugin, persisted by the server. */
export const config = {
  get: (key: string) => host.configGet(key),
  set: (key: string, value: string) => host.configSet(key, value),
  all: () => host.configAll(),
};

/**
 * Assert a capability was granted.
 *
 * Call at the top of a hook so a missing declaration produces an actionable
 * message at the point of use rather than a confusing failure later.
 */
export function requireCapability(name: Capability): void {
  if (!host.capabilities.includes(name)) {
    throw new Error(
      `plugin ${host.pluginId()} needs the "${name}" capability. ` +
        `Add it to the "capabilities" array in plugin.json. ` +
        `Granted: [${host.capabilities.join(", ")}]`,
    );
  }
}

/**
 * Declare a plugin.
 *
 * Purely for editor support and an id check: the host already read the
 * manifest, so the returned object is what it uses.
 */
export function definePlugin<T extends PluginHooks>(hooks: T): T {
  return hooks;
}

/** Install the bridge. Called by the host, never by plugins. */
export function install(bridge: HostBridge, capabilities: Capability[]): void {
  for (const [name, fn] of Object.entries(bridge)) {
    (host as unknown as Record<string, unknown>)[name] = fn;
  }
  host.capabilities = capabilities;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Ticks (100ns) to seconds. */
export function ticksToSeconds(ticks: number | undefined | null): number {
  return (ticks ?? 0) / 10_000_000;
}

/** Seconds to ticks (100ns). */
export function secondsToTicks(seconds: number): number {
  return Math.round(seconds * 10_000_000);
}

/**
 * `fetch` with a deadline.
 *
 * `AbortSignal.timeout` is the whole point: a plugin that hangs would otherwise
 * be killed by the host with its reply discarded, and anything the caller
 * already awaited is lost.
 */
export async function fetchWithTimeout(
  url: string,
  init: RequestInit & { timeoutMs?: number } = {},
): Promise<Response> {
  const { timeoutMs = 8_000, signal, ...rest } = init;
  const timeout = AbortSignal.timeout(timeoutMs);

  // Honour a caller-supplied signal as well as the timeout.
  const combined = signal ? AbortSignal.any([signal, timeout]) : timeout;

  return fetch(url, { ...rest, signal: combined });
}

/** GET a URL and parse JSON, with a deadline. */
export async function fetchJson<T = unknown>(
  url: string,
  init: RequestInit & { timeoutMs?: number } = {},
): Promise<T> {
  const response = await fetchWithTimeout(url, init);
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    throw new Error(`HTTP ${response.status} from ${url}: ${body.slice(0, 200)}`);
  }
  return (await response.json()) as T;
}