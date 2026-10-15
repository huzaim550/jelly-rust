import { useEffect, useRef, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { hrefFor, navigate, useRoute, type Route } from "../router";
import type { BaseItem } from "../types";
import { ChevronIcon, DashboardIcon, KeyIcon, ShelvesIcon, SignOutIcon, UsersIcon } from "./icons";

/**
 * Persistent header.
 *
 * Over the home hero it starts as bare glass and fills in once the page has
 * scrolled; everywhere else it is opaque. The media links mirror the sidebar
 * so the two stay in step, and search is debounced so typing does not fire a
 * request per keystroke.
 */
export function TopBar() {
  const { session, logout } = useSession();
  const [route] = useRoute();
  const [term, setTerm] = useState("");
  const [scrolled, setScrolled] = useState(false);
  const [views, setViews] = useState<BaseItem[]>([]);
  const [menuOpen, setMenuOpen] = useState(false);
  const profileRef = useRef<HTMLDivElement>(null);
  const userId = session?.user.Id ?? "";
  const serverName = session?.server.ServerName ?? "jellyfin-clone";

  // Fill the bar in as soon as the hero starts to leave.
  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 24);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    if (!userId) return;
    let cancelled = false;
    void api
      .views(userId)
      .then((items) => {
        if (!cancelled) setViews(items);
      })
      .catch(() => {
        // The sidebar owns surfacing this error; the bar just stays minimal.
      });
    return () => {
      cancelled = true;
    };
  }, [userId]);

  useEffect(() => {
    const trimmed = term.trim();
    if (!trimmed) return;

    const timer = window.setTimeout(() => {
      navigate({ name: "search", term: trimmed }, true);
    }, 300);

    return () => window.clearTimeout(timer);
  }, [term]);

  // Close the profile menu on outside click or Escape.
  useEffect(() => {
    if (!menuOpen) return;
    const onClick = (event: MouseEvent) => {
      if (!profileRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    window.addEventListener("mousedown", onClick);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onClick);
      window.removeEventListener("keydown", onKey);
    };
  }, [menuOpen]);

  function goHome() {
    navigate({ name: "home" });
    setTerm("");
  }

  function linkProps(target: Route) {
    return {
      href: hrefFor(target),
      onClick: (event: React.MouseEvent) => {
        // Keep SPA navigation and the back button working.
        event.preventDefault();
        navigate(target);
        setTerm("");
      },
    };
  }

  return (
    <header className={`topbar${scrolled ? " scrolled" : ""}`}>
      <button type="button" className="brand" onClick={goHome} aria-label={`${serverName} home`}>
        <span className="brand-mark" aria-hidden="true">
          <svg viewBox="0 0 28 28">
            <path d="M5 8.5 14 3l9 5.5v11L14 25l-9-5.5z" />
            <path d="m11 9 7 5-7 5z" />
          </svg>
        </span>
        <span className="brand-name">{serverName}</span>
      </button>

      {views.length > 0 && (
        <nav className="topbar-nav" aria-label="Media">
          <a
            className="topbar-nav-link"
            aria-current={route.name === "home" ? "page" : undefined}
            {...linkProps({ name: "home" })}
          >
            Home
          </a>
          {views.map((view) => (
            <a
              key={view.Id}
              className="topbar-nav-link"
              aria-current={
                route.name === "library" && route.viewId === view.Id ? "page" : undefined
              }
              {...linkProps({ name: "library", viewId: view.Id })}
            >
              {view.Name}
            </a>
          ))}
        </nav>
      )}

      <span className="spacer" />

      <input
        className="search"
        type="search"
        placeholder="Search titles…"
        aria-label="Search"
        value={term}
        onChange={(e) => setTerm(e.target.value)}
      />

      <div className="topbar-actions">
        <button
          type="button"
          className="icon-button"
          aria-label="API keys"
          title="API keys"
          onClick={() => navigate({ name: "api-keys" })}
        >
          <KeyIcon />
        </button>

        <div className="profile" ref={profileRef}>
          <button
            type="button"
            className="profile-button"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            aria-label="Account menu"
            onClick={() => setMenuOpen((value) => !value)}
          >
            <span className="profile-avatar" aria-hidden="true">
              {(session?.user.Name ?? "?").slice(0, 1).toUpperCase()}
            </span>
            <ChevronIcon className="profile-caret" />
          </button>

          {menuOpen && (
            <div className="profile-menu" role="menu">
              <div className="profile-menu-head">
                <strong>{session?.user.Name}</strong>
                <span>
                  {session?.user.Policy.IsAdministrator ? "Administrator" : "Member"} ·{" "}
                  {serverName}
                </span>
              </div>
              {session?.user.Policy.IsAdministrator && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setMenuOpen(false);
                    navigate({ name: "dashboard" });
                  }}
                >
                  <DashboardIcon />
                  Dashboard
                </button>
              )}
              {session?.user.Policy.IsAdministrator && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setMenuOpen(false);
                    navigate({ name: "libraries" });
                  }}
                >
                  <ShelvesIcon />
                  Libraries
                </button>
              )}
              {session?.user.Policy.IsAdministrator && (
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setMenuOpen(false);
                    navigate({ name: "users" });
                  }}
                >
                  <UsersIcon />
                  Users
                </button>
              )}
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMenuOpen(false);
                  navigate({ name: "api-keys" });
                }}
              >
                <KeyIcon />
                API keys
              </button>
              <button
                type="button"
                role="menuitem"
                className="danger"
                onClick={() => {
                  setMenuOpen(false);
                  void logout();
                }}
              >
                <SignOutIcon />
                Sign out
              </button>
            </div>
          )}
        </div>
      </div>
    </header>
  );
}
