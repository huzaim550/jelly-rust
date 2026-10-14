import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { libraryIcon, useLibraries } from "../library";
import { LibraryPicker } from "./Welcome";
import type { Route } from "../router";

/**
 * Library and plugin administration, reachable from the top bar.
 *
 * Admin-only in the UI, and the server enforces that too — a hidden route is
 * not the security boundary, so every handler here re-checks the policy.
 */
export function LibrarySettings({ go }: { go: (route: Route) => void }) {
  const { session } = useSession();
  const { folders, scanning, statuses, remove, rescan, refresh, error } = useLibraries();

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
            <button type="button" className="ghost" onClick={() => void rescan()} disabled={scanning}>
              {scanning ? "Scanning…" : "Scan all"}
            </button>
            <button type="button" className="link" onClick={() => void refresh()}>
              Refresh
            </button>
          </div>
        </div>

        {error && <p className="error">{error}</p>}

        {folders.length === 0 && !scanning && (
          <p className="muted">No libraries yet. Add one below.</p>
        )}

        {folders.map((folder) => {
          const status = statuses[folder.ItemId];
          return (
            <div key={folder.ItemId} className="library-row">
              <span className="icon">{libraryIcon(folder.CollectionType)}</span>
              <div className="info">
                <div className="name">{folder.Name}</div>
                <div className="muted small path">{folder.Locations.join(", ")}</div>
                {status?.state === "Scanning" && (
                  <>
                    <div className="bar slim">
                      <div
                        className="fill"
                        style={{ width: `${Math.round((status.progress ?? 0) * 100)}%` }}
                      />
                    </div>
                    <div className="muted small">
                      {Math.round((status.progress ?? 0) * 100)}% · {status.processed} files
                    </div>
                  </>
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
