import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { useLibraries } from "../library";
import type { Route } from "../router";

/**
 * Server overview: identity, control and health, reachable from the top bar.
 *
 * Library *management* lives on its own screen (`/settings/libraries`) so this
 * page stays read-only. Admin-only in the UI, and the server enforces that too
 * — a hidden route is not the security boundary, so every handler here
 * re-checks the policy.
 */
export function Dashboard({ go }: { go: (route: Route) => void }) {
  const { session } = useSession();
  // Read-only here: the counts below, and nothing that mutates a library.
  const { folders, scanning } = useLibraries();
  const [sessions, setSessions] = useState<
    Array<{
      Id: string;
      UserName: string;
      Client: string;
      DeviceName: string;
      ApplicationVersion: string;
      IsActive: boolean;
    }>
  >([]);
  const [sessionsError, setSessionsError] = useState("");
  const [sessionsLoaded, setSessionsLoaded] = useState(false);
  const [serverAction, setServerAction] = useState<"restart" | "shutdown" | null>(null);
  const [serverActionMessage, setServerActionMessage] = useState("");

  useEffect(() => {
    if (!session?.user.Policy.IsAdministrator) return;
    api
      .sessions()
      .then((activeSessions) => {
        setSessions(activeSessions.filter((entry) => entry.IsActive));
        setSessionsError("");
      })
      .catch((err: unknown) => {
        setSessionsError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => setSessionsLoaded(true));
  }, [session?.user.Policy.IsAdministrator]);

  async function controlServer(action: "restart" | "shutdown") {
    const verb = action === "restart" ? "restart" : "shut down";
    if (!window.confirm(`Are you sure you want to ${verb} the server?`)) return;

    setServerAction(action);
    setServerActionMessage("");
    try {
      await api.serverControl(action);
      setServerActionMessage(
        action === "restart"
          ? "Restarting the server. It will be available again shortly."
          : "The server is shutting down.",
      );
    } catch (err) {
      setServerAction(null);
      setServerActionMessage(err instanceof Error ? err.message : String(err));
    }
  }

  if (!session?.user.Policy.IsAdministrator) {
    return (
      <main>
        <p className="error">The dashboard requires administrator access.</p>
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

      <div className="dashboard-layout">
        <div className="dashboard-primary">
          <section className="server-card">
            <h2>Server</h2>
            <dl>
              <dt>Server name</dt>
              <dd>{session.server.ServerName}</dd>
              <dt>Server version</dt>
              <dd>{session.server.Version || "Development"}</dd>
              <dt>Account</dt>
              <dd>{session.user.Name}</dd>
            </dl>
          </section>

          <div className="dashboard-actions">
            <button
              type="button"
              className="danger-button"
              onClick={() => void controlServer("restart")}
              disabled={serverAction !== null}
            >
              Restart
            </button>
            <button
              type="button"
              className="danger-button"
              onClick={() => void controlServer("shutdown")}
              disabled={serverAction !== null}
            >
              Shutdown
            </button>
          </div>
          {serverActionMessage && (
            <p
              className={serverAction ? "server-action-message" : "error"}
              role="status"
            >
              {serverActionMessage}
            </p>
          )}

          <div className="dashboard-stats" aria-label="Server summary">
            <div className="dashboard-stat">
              <span>Libraries</span>
              <strong>{folders.length}</strong>
            </div>
            <div className="dashboard-stat">
              <span>Active devices</span>
              <strong>{sessions.length}</strong>
            </div>
            <div className="dashboard-stat">
              <span>Scan status</span>
              <strong className="stat-status">{scanning ? "Scanning" : "Idle"}</strong>
            </div>
          </div>

          <p className="dashboard-manage">
            <button type="button" className="link" onClick={() => go({ name: "libraries" })}>
              Manage libraries →
            </button>
          </p>

          <section>
            <h2>Plugins</h2>
            <PluginList />
          </section>
        </div>

        <aside className="activity-panel">
          <h2>Activity</h2>
          {sessionsError && <p className="error">{sessionsError}</p>}
          {!sessionsLoaded && <p className="muted">Loading active sessions…</p>}
          {sessionsLoaded && !sessionsError && sessions.length === 0 && (
            <p className="muted">No other active devices.</p>
          )}
          {sessions.map((entry) => (
            <div className="activity-entry" key={entry.Id}>
              <span className="activity-mark" aria-hidden="true">●</span>
              <div>
                <strong>{entry.UserName}</strong>
                <span> is connected from {entry.Client || entry.DeviceName || "a device"}</span>
                {entry.ApplicationVersion && (
                  <small>Client version {entry.ApplicationVersion}</small>
                )}
              </div>
            </div>
          ))}
        </aside>
      </div>
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
        No plugins loaded. Check the server startup logs and <code>JF_PLUGINS_DIR</code>.
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
