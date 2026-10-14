import { useEffect, useState } from "react";
import { useSession } from "../App";
import type { Route } from "../router";

/**
 * Persistent header: server name, search, and sign-out.
 *
 * Search is debounced so typing does not fire a request per keystroke.
 */
export function TopBar() {
  const { session, logout } = useSession();
  const [term, setTerm] = useState("");
  const [, setRoute] = useState<Route>({ name: "home" });

  useEffect(() => {
    const trimmed = term.trim();
    if (!trimmed) return;

    const timer = window.setTimeout(() => {
      window.history.replaceState({}, "", `/search?q=${encodeURIComponent(trimmed)}`);
      setRoute({ name: "search", term: trimmed });
    }, 300);

    return () => window.clearTimeout(timer);
  }, [term]);

  function goHome() {
    window.history.pushState({}, "", "/");
    setRoute({ name: "home" });
    setTerm("");
  }

  return (
    <header className="topbar">
      <button type="button" className="brand" onClick={goHome}>
        {session?.server.ServerName ?? "jellyfin-clone"}
      </button>

      <input
        className="search"
        type="search"
        placeholder="Search…"
        aria-label="Search"
        value={term}
        onChange={(e) => setTerm(e.target.value)}
      />

      <span className="spacer" />

      {session?.user.Name && <span className="muted username">{session.user.Name}</span>}

      {session?.user.Policy.IsAdministrator && (
        <button
          type="button"
          className="link"
          onClick={() => {
            window.history.pushState({}, "", "/dashboard");
            setRoute({ name: "dashboard" });
          }}
        >
          Dashboard
        </button>
      )}

      <button type="button" className="link" onClick={() => void logout()}>
        Sign out
      </button>
    </header>
  );
}