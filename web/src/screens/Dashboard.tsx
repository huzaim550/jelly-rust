import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { PosterGrid } from "../components/ItemCard";
import {
  libraryIcon,
  libraryLabel,
  useLibraries,
} from "../library";
import { FolderList, LibraryPicker } from "./Welcome";
import type { BaseItem } from "../types";
import type { Route } from "../router";

/**
 * Home dashboard.
 *
 * Jellyfin's shape: library tiles first, then resume and next-up rows. When a
 * scan is running, progress replaces the tiles so the user knows why the counts
 * are moving.
 */
export function Home({
  go,
  onPlay,
}: {
  go: (route: Route, replace?: boolean) => void;
  onPlay: (item: { itemId: string; title: string }) => void;
}) {
  const { session } = useSession();
  const userId = session?.user.Id ?? "";
  const { folders, scanning, statuses, error } = useLibraries();

  const [resume, setResume] = useState<BaseItem[]>([]);
  const [nextUp, setNextUp] = useState<BaseItem[]>([]);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    if (!userId) return;
    let cancelled = false;

    (async () => {
      try {
        // Independent requests so one slow endpoint does not blank the page.
        const [r, n] = await Promise.allSettled([
          api.resume(userId, 12),
          api.nextUp(userId, 12),
        ]);
        if (cancelled) return;
        if (r.status === "fulfilled") setResume(r.value.Items);
        if (n.status === "fulfilled") setNextUp(n.value.Items);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [userId]);


  // A finished scan changes what is on the dashboard, so reload when the
  // scanning flag flips back to false.
  useEffect(() => {
    if (scanning || !userId) return;
    void api.resume(userId, 12).then((r) => setResume(r.Items)).catch(() => {});
  }, [scanning, userId]);

  const openItem = (item: BaseItem) => go({ name: "item", itemId: item.Id });
  const playItem = (item: BaseItem) => onPlay({ itemId: item.Id, title: item.Name });

  return (
    <main>
      {error && <p className="error">{error}</p>}

      {scanning ? (
        <ScanPanel folders={folders} statuses={statuses} />
      ) : (
        <section>
          <h2>My Libraries</h2>
          {folders.length === 0 ? (
            <div className="empty">
              <p>No libraries yet.</p>
              <p className="muted">
                Add one from the dashboard settings, or run{" "}
                <code>jf-admin add-library /path/to/media movies</code>.
              </p>
            </div>
          ) : (
            <div className="grid views">
              {folders.map((folder) => (
                <button
                  key={folder.ItemId}
                  type="button"
                  className="card view"
                  onClick={() => go({ name: "library", viewId: folder.ItemId })}
                >
                  <span className="view-icon" aria-hidden="true">
                    {libraryIcon(folder.CollectionType)}
                  </span>
                  <span className="view-name">{folder.Name}</span>
                  <span className="muted small">
                    {libraryLabel(folder.CollectionType)}
                  </span>
                </button>
              ))}
            </div>
          )}
        </section>
      )}

      {resume.length > 0 && (
        <section>
          <h2>Continue Watching</h2>
          <PosterGrid items={resume} onOpen={openItem} onPlay={playItem} />
        </section>
      )}

      {nextUp.length > 0 && (
        <section>
          <h2>Next Up</h2>
          <PosterGrid items={nextUp} onOpen={openItem} onPlay={playItem} />
        </section>
      )}

      {loading && <p className="muted loading">Loading…</p>}
    </main>
  );
}

/** Shown in place of the tiles while a scan runs. */
function ScanPanel({
  folders,
  statuses,
}: {
  folders: Array<{ ItemId: string; Name: string; CollectionType: string | null }>;
  statuses: Record<string, { state: string; progress: number | null; detail: string | null }>;
}) {
  return (
    <section>
      <h2>Scanning your libraries…</h2>
      <ul className="scan-list">
        {folders.map((folder) => {
          const status = statuses[folder.ItemId];
          const progress = status?.progress ?? 0;
          return (
            <li key={folder.ItemId}>
              <span className="name">{folder.Name}</span>
              <div className="bar">
                <div className="fill" style={{ width: `${Math.round(progress * 100)}%` }} />
              </div>
              <span className="muted small">
                {status?.state === "Scanning"
                  ? `${Math.round(progress * 100)}%`
                  : (status?.state ?? "Waiting")}
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/**
 * Library and plugin administration, reachable from the top bar.
 *
 * Admin-only; the server enforces that too, so a hidden route is not the
 * security boundary.
 */
export function LibrarySettings({ go }: { go: (route: Route) => void }) {
  const { session } = useSession();
  const { folders, scanning, statuses, remove, rescan, refresh } = useLibraries();

  if (!session?.user.Policy.IsAdministrator) {
    return (
      <main>
        <p className="error">Library settings require administrator access.</p>
      </main>
    );
  }

  return (
    <main>
      <div className="section-head">
        <h1>Dashboard</h1>
        <button type="button" className="link" onClick={() => go({ name: "home" })}>
          ← Back
        </button>
      </div>

      <section>
        <div className="section-head">
          <h2>Libraries</h2>
          <div className="row-actions">
            <button
              type="button"
              className="ghost"
              onClick={() => void rescan()}
              disabled={scanning}
            >
              {scanning ? "Scanning…" : "Scan all"}
            </button>
            <button type="button" className="link" onClick={() => void refresh()}>
              Refresh
            </button>
          </div>
        </div>

        {folders.map((folder) => {
          const status = statuses[folder.ItemId];
          return (
            <div key={folder.ItemId} className="library-row">
              <span className="icon">{libraryIcon(folder.CollectionType)}</span>
              <div className="info">
                <div className="name">{folder.Name}</div>
                <div className="muted small path">{folder.Locations.join(", ")}</div>
                {status?.state === "Scanning" && (
                  <div className="bar slim">
                    <div
                      className="fill"
                      style={{ width: `${Math.round((status.progress ?? 0) * 100)}%` }}
                    />
                  </div>
                )}
                {status?.detail && <div className="muted small">{status.detail}</div>}
              </div>
              <button
                type="button"
                className="link danger"
                onClick={() => void remove(folder.Name)}
              >
                Remove
              </button>
            </div>
          );
        })}

        <FolderList folders={folders} onChange={() => void refresh()} />
      </section>

      <section>
        <h2>Add a library</h2>
        <LibraryPicker onAdded={() => void refresh()} />
      </section>

      <section>
        <h2>Plugins</h2>
        <PluginList />
      </section>
    </main>
  );
}

function PluginList() {
  const [plugins, setPlugins] = useState<
    Array<{ id: string; name: string; version: string; metadataProviders: string[] }>
  >([]);
  const [failed, setFailed] = useState<Array<{ id: string; reason: string }>>([]);
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    api
      .plugins()
      .then((data) => {
        setPlugins(data.Report?.loaded ?? []);
        setFailed(data.Report?.failed ?? []);
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoaded(true));
  }, []);

  if (error) return <p className="error">{error}</p>;
  if (!loaded) return <p className="muted">Loading…</p>;

  if (plugins.length === 0 && failed.length === 0) {
    return (
      <p className="muted">
        No plugins installed. Put one in <code>&lt;data&gt;/plugins/</code>.
      </p>
    );
  }

  return (
    <ul className="folder-list">
      {plugins.map((plugin) => (
        <li key={plugin.id}>
          <span className="name">{plugin.name}</span>
          <span className="muted">v{plugin.version}</span>
          {plugin.metadataProviders.length > 0 && (
            <span className="chip">{plugin.metadataProviders.join(", ")}</span>
          )}
        </li>
      ))}
      {failed.map((f) => (
        <li key={f.id} className="failed">
          <span className="name">{f.id}</span>
          <span className="error small">{f.reason}</span>
        </li>
      ))}
    </ul>
  );
}