/**
 * Plugin loading and request dispatch.
 *
 * Discovers plugins under a directory, imports them, and serves the RPC the
 * Rust supervisor speaks. The protocol is newline-delimited JSON on stdio:
 * one request object per line out, one response object per line in.
 *
 * Requests carry an `id`; responses echo it so the supervisor can correlate.
 * Log lines are unsolicited and carry no id.
 */

import { readdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { createInterface } from "node:readline";

import {
  API_VERSION,
  install,
  type Capability,
  type ItemSummary,
  type MetadataQuery,
  type MetadataResult,
  type PluginEvent,
  type PluginHooks,
} from "./sdk.js";

/** Hook deadline. Must sit under the supervisor's own timeout so we answer
 * first and the reply is not discarded. */
const HOOK_TIMEOUT_MS = 12_000;

interface Manifest {
  id: string;
  name: string;
  version: string;
  description?: string;
  author?: string;
  apiVersion: string;
  entry: string;
  capabilities?: Capability[];
  metadataProviders?: string[];
  dependsOn?: string[];
}

interface LoadedPlugin {
  manifest: Manifest;
  hooks: PluginHooks;
  dir: string;
}

interface LoadReport {
  loaded: Array<{
    id: string;
    name: string;
    version: string;
    metadataProviders: string[];
    capabilities: Capability[];
  }>;
  failed: Array<{ id: string; reason: string }>;
  apiVersion: string;
}

/** Requests from the supervisor. */
type HostRequest =
  | { id?: number; type: "load"; pluginsDir: string }
  | { id?: number; type: "fetchMetadata"; plugin: string; query: MetadataQuery; timeoutMs: number }
  | { type: "event"; event: PluginEvent }
  | { type: "shutdown" };

/** Responses back to the supervisor. */
type HostResponse =
  | { type: "loaded"; report: LoadReport }
  | { type: "result"; id: number; ok: boolean; value?: unknown; error?: string }
  | { type: "log"; record: { level: string; pluginId: string; message: string } }
  | { type: "exited"; code: number | null };

const plugins = new Map<string, LoadedPlugin>();

/** Bridge the SDK talks to. Replaced per plugin as each one is imported. */
function bridgeFor(pluginId: string, capabilities: Capability[], currentId: () => string) {
  return {
    log(level: string, message: string): void {
      send({ type: "log", record: { level, pluginId, message } });
    },
    async getItem(itemId: string): Promise<ItemSummary | undefined> {
      // Item reads go through the supervisor, which owns the database. There is
      // no direct path from here, and adding one would widen the capability
      // surface for every plugin in the process.
      return (await request<ItemSummary | null>("getItem", { itemId })) ?? undefined;
    },
    apiVersion: () => API_VERSION,
    pluginId: () => currentId(),
    async configGet(key: string): Promise<string | undefined> {
      const value = await request<string | null>("configGet", { pluginId, key });
      return value ?? undefined;
    },
    async configSet(key: string, value: string): Promise<void> {
      await request<null>("configSet", { pluginId, key, value });
    },
    async configAll(): Promise<Record<string, string>> {
      return (await request<Record<string, string>>("configAll", { pluginId })) ?? {};
    },
  };
}

function send(message: HostResponse): void {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

/** Which plugin's bridge is currently installed, for `pluginId()`. */
let activePlugin = "";

/**
 * Calls back into the supervisor.
 *
 * Only item reads and config reach for this. Those are rare in a plugin — a
 * provider usually has everything it needs in the query — so the extra round
 * trip is acceptable, and it keeps the plugin process unable to touch the
 * database directly.
 */
async function request<T>(type: string, payload: Record<string, unknown>): Promise<T | null> {
  return new Promise<T | null>((resolve) => {
    const id = nextRequestId();
    pending.set(id, (value) => resolve(value as T));
    process.stdout.write(`${JSON.stringify({ type, id, ...payload })}\n`);
  });
}

let requestSeq = 1_000_000;
const pending = new Map<number, (value: unknown) => void>();

/**
 * Correlation id for a plugin-initiated request.
 *
 * Starts high so it cannot collide with supervisor-issued ids, which begin at
 * 1 on the other side of the process boundary.
 */
function nextRequestId(): number {
  requestSeq += 1;
  return requestSeq;
}

// -----------------------------------------------------------------------
// Loading
// -----------------------------------------------------------------------

async function loadPlugins(dir: unknown): Promise<LoadReport> {
  const report: LoadReport = { loaded: [], failed: [], apiVersion: API_VERSION };

  // Guard the type rather than trusting the peer: `existsSync(undefined)`
  // throws in current Node, and the resulting unhandled rejection would leave
  // the supervisor waiting until its own deadline.
  if (typeof dir !== "string" || dir.length === 0) {
    report.failed.push({ id: "host", reason: "load request had no pluginsDir" });
    return report;
  }

  if (!existsSync(dir)) {
    return report;
  }

  const entries = await readdir(dir, { withFileTypes: true });
  const pluginDirs = entries.filter((e) => e.isDirectory()).map((e) => e.name).sort();

  const manifests: Array<{ dir: string; manifest: Manifest }> = [];

  for (const name of pluginDirs) {
    const pluginDir = path.join(dir, name);
    const manifestPath = path.join(pluginDir, "plugin.json");

    if (!existsSync(manifestPath)) {
      report.failed.push({ id: name, reason: "missing plugin.json" });
      continue;
    }

    let manifest: Manifest;
    try {
      manifest = JSON.parse(await readFile(manifestPath, "utf8")) as Manifest;
    } catch (error) {
      report.failed.push({ id: name, reason: `invalid plugin.json: ${String(error)}` });
      continue;
    }

    const problem = validate(manifest);
    if (problem) {
      report.failed.push({ id: manifest.id ?? name, reason: problem });
      continue;
    }

    manifests.push({ dir: pluginDir, manifest });
  }

  // Import in dependency order: a plugin that lists a dependency must not be
  // loaded before it.
  for (const { dir: pluginDir, manifest } of manifests) {
    const missing = (manifest.dependsOn ?? []).filter((d) => !plugins.has(d));
    if (missing.length > 0) {
      report.failed.push({
        id: manifest.id,
        reason: `missing dependencies: ${missing.join(", ")}`,
      });
      continue;
    }

    try {
      const entryPath = path.join(pluginDir, manifest.entry);
      if (!existsSync(entryPath)) {
        throw new Error(`entry not found: ${manifest.entry}`);
      }

      // Node resolves a plugin's own dependencies from its package.json, so a
      // plugin with `node_modules` next to it gets full npm support.
      const module = (await import(pathToFileURL(entryPath).href)) as {
        default?: PluginHooks;
        setup?: PluginHooks["setup"];
        fetchMetadata?: PluginHooks["fetchMetadata"];
        onEvent?: PluginHooks["onEvent"];
        teardown?: PluginHooks["teardown"];
      };

      const hooks: PluginHooks = module.default ?? {
        setup: module.setup,
        fetchMetadata: module.fetchMetadata,
        onEvent: module.onEvent,
        teardown: module.teardown,
      };

      if (typeof hooks.fetchMetadata !== "function" && typeof hooks.onEvent !== "function") {
        throw new Error("module exports no fetchMetadata or onEvent hook");
      }

      const capabilities = manifest.capabilities ?? [];

      // Install a bridge scoped to this plugin, then run setup. The bridge is
      // per-plugin because `pluginId()` must report the right one, and plugins
      // are re-entrant.
      activePlugin = manifest.id;
      install(bridgeFor(manifest.id, capabilities, () => activePlugin), capabilities);

      if (hooks.setup) {
        await withTimeout(Promise.resolve(hooks.setup()), 5_000, "setup");
      }

      plugins.set(manifest.id, { manifest, hooks, dir: pluginDir });

      report.loaded.push({
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        metadataProviders: manifest.metadataProviders ?? [],
        capabilities,
      });
    } catch (error) {
      report.failed.push({ id: manifest.id, reason: describe(error) });
    }
  }

  return report;
}

function validate(manifest: Manifest): string | null {
  if (!manifest.id || !/^[\w.-]+$/.test(manifest.id)) {
    return `illegal plugin id: ${manifest.id}`;
  }
  if (!manifest.name) return "plugin.json is missing `name`";
  if (!manifest.version) return "plugin.json is missing `version`";
  if (!manifest.entry) return "plugin.json is missing `entry`";

  if (!manifest.apiVersion?.startsWith("1.")) {
    return `targets host API ${manifest.apiVersion}, but this host speaks ${API_VERSION}`;
  }
  return null;
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

/** Reject if a hook outruns its budget. */
async function withTimeout<T>(work: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      work,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${what} exceeded ${ms}ms`)), ms);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

// -----------------------------------------------------------------------
// Dispatch
// -----------------------------------------------------------------------

async function handle(request: HostRequest): Promise<void> {
  switch (request.type) {
    case "load": {
      const report = await loadPlugins(request.pluginsDir);
      // Answer with a correlated `result` so the supervisor's request resolves.
      // A bespoke message here would be ignored and the caller would sit until
      // its deadline.
      reply(request.id, true, report);
      return;
    }

    case "fetchMetadata": {
      const plugin = plugins.get(request.plugin);
      if (!plugin) {
        reply(request.id, false, undefined, `plugin "${request.plugin}" is not loaded`);
        return;
      }
      if (!plugin.hooks.fetchMetadata) {
        reply(request.id, true, null);
        return;
      }

      // Re-install the bridge so `pluginId()` is correct during the call.
      activePlugin = plugin.manifest.id;
      install(
        bridgeFor(plugin.manifest.id, plugin.manifest.capabilities ?? [], () => activePlugin),
        plugin.manifest.capabilities ?? [],
      );

      try {
        // Stay under the supervisor's deadline so our reply is the one that
        // counts, rather than being discarded as a timeout.
        const budget = Math.max(1_000, Math.min(request.timeoutMs, HOOK_TIMEOUT_MS) - 1_000);
        const result = await withTimeout(
          Promise.resolve(plugin.hooks.fetchMetadata(request.query)),
          budget,
          "fetchMetadata",
        );
        // `undefined` means "I have nothing"; the chain should continue.
        reply(request.id, true, result ?? null);
      } catch (error) {
        reply(request.id, false, undefined, describe(error));
      }
      return;
    }

    case "event": {
      // Fan out to every subscriber. A throwing handler is logged and skipped
      // so one bad plugin cannot silence the rest.
      for (const plugin of plugins.values()) {
        if (!plugin.hooks.onEvent) continue;

        activePlugin = plugin.manifest.id;
        install(
          bridgeFor(plugin.manifest.id, plugin.manifest.capabilities ?? [], () => activePlugin),
          plugin.manifest.capabilities ?? [],
        );

        try {
          await withTimeout(
            Promise.resolve(plugin.hooks.onEvent(request.event)),
            5_000,
            "onEvent",
          );
        } catch (error) {
          send({
            type: "log",
            record: {
              level: "error",
              pluginId: plugin.manifest.id,
              message: `event handler failed: ${describe(error)}`,
            },
          });
        }
      }
      return;
    }

    case "shutdown": {
      for (const plugin of plugins.values()) {
        try {
          await withTimeout(Promise.resolve(plugin.hooks?.teardown?.()), 2_000, "teardown");
        } catch {
          // Shutdown is best effort.
        }
      }
      process.exit(0);
    }

    // Plugin-initiated requests (getItem, config*) are answered by the
    // supervisor and arrive back as `result`, which the line handler resolves.
    // They never reach this switch.
  }
}

function reply(id: number | undefined, ok: boolean, value?: unknown, error?: string): void {
  if (id === undefined) return;
  send({ type: "result", id, ok, value, error });
}

// -----------------------------------------------------------------------
// Wiring
// -----------------------------------------------------------------------

/**
 * One reader for stdin, serving both directions.
 *
 * The supervisor sends requests (`load`, `fetchMetadata`, ...) and also replies
 * to requests a plugin initiated (`configGet`, `getItem`). Both arrive on the
 * same stream, so a single line handler must dispatch on `type`; attaching a
 * second listener would race it for bytes.
 */
const rl = createInterface({ input: process.stdin });

rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;

  let message: (HostRequest & { id?: number }) | SupervisorReply;
  try {
    message = JSON.parse(trimmed);
  } catch (error) {
    send({
      type: "log",
      record: { level: "error", pluginId: "host", message: `bad message: ${String(error)}` },
    });
    return;
  }

  // A reply to something a plugin asked for.
  if (message.type === "result") {
    const reply = message as SupervisorReply;
    const resolve = pending.get(reply.id);
    if (resolve) {
      pending.delete(reply.id);
      if (!reply.ok) {
        resolve(undefined);
      } else {
        resolve(reply.value);
      }
    }
    return;
  }

  const request = message as HostRequest & { id?: number };
  void handle(request).catch((error) => {
    reply(request.id, false, undefined, describe(error));
  });
});

/** A reply to a plugin-initiated request. */
interface SupervisorReply {
  type: "result";
  id: number;
  ok: boolean;
  value?: unknown;
  error?: string;
}

process.on("uncaughtException", (error) => {
  send({ type: "log", record: { level: "error", pluginId: "host", message: describe(error) } });
});

process.on("unhandledRejection", (reason) => {
  send({
    type: "log",
    record: { level: "error", pluginId: "host", message: `unhandled rejection: ${describe(reason)}` },
  });
});

// Confirm liveness so the supervisor can tell "starting" from "wedged".
send({
  type: "log",
  record: { level: "info", pluginId: "host", message: `plugin host ready (API ${API_VERSION})` },
});

export { loadPlugins, handle };