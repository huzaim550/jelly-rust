import { useSession } from "../App";
import { libraryIcon, useLibraries } from "../library";
import { LibraryPicker } from "./Welcome";
import type { Route } from "../router";

/**
 * Library management: the media sources themselves.
 *
 * Split out of the dashboard so that screen stays a read-only overview of the
 * server. Admin-only in the UI, and the server enforces that too — a hidden
 * route is not the security boundary, so the requests still carry the token
 * and the server decides.
 */
export function Libraries({ go }: { go: (route: Route) => void }) {
  const { session } = useSession();
  const isAdmin = session?.user.Policy.IsAdministrator ?? false;
  const { folders, scanning, statuses, remove, rescan, refresh, error } = useLibraries(isAdmin);

  if (!isAdmin) {
    return (
      <main>
        <p className="error">Library management requires administrator access.</p>
      </main>
    );
  }

  return (
    <main>
      <div className="section-head">
        <h1>Libraries</h1>
        <button type="button" className="link" onClick={() => go({ name: "home" })}>
          ← Back
        </button>
      </div>

      <div className="settings-page">
        <div className="dashboard-actions">
          <button
            type="button"
            className="primary"
            onClick={() => void rescan()}
            disabled={scanning}
          >
            {scanning ? "Scanning libraries…" : "Scan all libraries"}
          </button>
          <button type="button" className="link" onClick={() => void refresh()}>
            Refresh
          </button>
        </div>

        {error && <p className="error">{error}</p>}

        <section>
          <h2>Media libraries</h2>

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
      </div>
    </main>
  );
}
