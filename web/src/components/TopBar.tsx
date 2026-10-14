import { useEffect, useState } from "react";
import { useSession } from "../App";
import { navigate } from "../router";

/**
 * Persistent header: server name, search, and sign-out.
 *
 * Search is debounced so typing does not fire a request per keystroke.
 */
export function TopBar() {
  const { session, logout } = useSession();
  const [term, setTerm] = useState("");

  useEffect(() => {
    const trimmed = term.trim();
    if (!trimmed) return;

    const timer = window.setTimeout(() => {
      navigate({ name: "search", term: trimmed }, true);
    }, 300);

    return () => window.clearTimeout(timer);
  }, [term]);

  function goHome() {
    navigate({ name: "home" });
    setTerm("");
  }

  return (
    <header className="topbar">
      <button type="button" className="brand" onClick={goHome}>
        <span className="brand-mark" aria-hidden="true">
          <svg viewBox="0 0 28 28">
            <path d="M5 8.5 14 3l9 5.5v11L14 25l-9-5.5z" />
            <path d="m11 9 7 5-7 5z" />
          </svg>
        </span>
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

      <button type="button" className="link" onClick={() => void logout()}>
        Sign out
      </button>
    </header>
  );
}