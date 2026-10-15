import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { navigate, type Route } from "../router";
import type { BaseItem } from "../types";
import { CompassIcon, DashboardIcon, KeyIcon, LibraryIcon, ShelvesIcon, UsersIcon } from "./icons";

/**
 * Primary navigation.
 *
 * On the admin screens it stays docked beside the workspace; on the home screen
 * it collapses behind a hamburger so the hero owns the full width.
 */
export function Sidebar({ route }: { route: Route }) {
  const { session } = useSession();
  const userId = session?.user.Id ?? "";
  const [views, setViews] = useState<BaseItem[]>([]);
  const [viewsError, setViewsError] = useState("");
  const [open, setOpen] = useState(false);
  const collapsed = route.name === "home";

  useEffect(() => {
    setOpen(false);
  }, [route]);

  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [open]);

  useEffect(() => {
    let cancelled = false;
    if (!userId) return;
    void api
      .views(userId)
      .then((items) => {
        if (!cancelled) {
          setViews(items);
          setViewsError("");
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setViews([]);
          setViewsError(err instanceof Error ? err.message : String(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [userId]);

  const active = (target: Route) =>
    route.name === target.name &&
    (target.name !== "library" ||
      (route.name === "library" && route.viewId === target.viewId));

  const sidebar = (
    <aside className={`sidebar${collapsed ? " sidebar-drawer" : ""}`}>
      <button
        type="button"
        className="sidebar-brand"
        onClick={() => navigate({ name: "home" })}
      >
        <span className="brand-mark" aria-hidden="true">
          <svg viewBox="0 0 28 28">
            <path d="M5 8.5 14 3l9 5.5v11L14 25l-9-5.5z" />
            <path d="m11 9 7 5-7 5z" />
          </svg>
        </span>
        <span className="sidebar-brand-copy">
          <strong>{session?.server.ServerName ?? "jellyfin-clone"}</strong>
          {session?.server.Version && <small>Version {session.server.Version}</small>}
        </span>
      </button>

      <nav aria-label="Main navigation">
        <div className="sidebar-group">
          <span className="sidebar-label">Browse</span>
          <SidebarLink
            label="Browse"
            icon={<CompassIcon />}
            selected={active({ name: "home" })}
            onClick={() => navigate({ name: "home" })}
          />
          {views.map((view) => (
            <SidebarLink
              key={view.Id}
              label={view.Name}
              icon={<LibraryIcon />}
              nested
              selected={active({ name: "library", viewId: view.Id })}
              onClick={() => navigate({ name: "library", viewId: view.Id })}
            />
          ))}
        </div>

        <div className="sidebar-group">
          <span className="sidebar-label">Account</span>
          {session?.user.Policy.IsAdministrator && (
            <>
              <SidebarLink
                label="Dashboard"
                icon={<DashboardIcon />}
                selected={active({ name: "dashboard" })}
                onClick={() => navigate({ name: "dashboard" })}
              />
              <SidebarLink
                label="Users"
                icon={<UsersIcon />}
                selected={active({ name: "users" })}
                onClick={() => navigate({ name: "users" })}
              />
            </>
          )}
          {session?.user.Policy.IsAdministrator && (
            <SidebarLink
              label="Libraries"
              icon={<ShelvesIcon />}
              selected={active({ name: "libraries" })}
              onClick={() => navigate({ name: "libraries" })}
            />
          )}
          <SidebarLink
            label="API Keys"
            icon={<KeyIcon />}
            selected={active({ name: "api-keys" })}
            onClick={() => navigate({ name: "api-keys" })}
          />
        </div>
        {viewsError && (
          <p className="sidebar-error" role="status">
            {viewsError}
          </p>
        )}
      </nav>

      <div className="sidebar-account">
        <span className="sidebar-user-mark" aria-hidden="true">
          {(session?.user.Name ?? "?").slice(0, 1).toUpperCase()}
        </span>
        <span className="sidebar-user-name">{session?.user.Name}</span>
      </div>
    </aside>
  );

  if (!collapsed) return sidebar;

  return (
    <>
      <button
        type="button"
        className="sidebar-menu-toggle"
        aria-label="Open navigation menu"
        aria-expanded={open}
        aria-controls="primary-navigation"
        onClick={() => setOpen((value) => !value)}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M4 6h16M4 12h16M4 18h16" />
        </svg>
      </button>
      {open && (
        <>
          <button
            type="button"
            className="sidebar-backdrop"
            aria-label="Close navigation menu"
            onClick={() => setOpen(false)}
          />
          <div id="primary-navigation">{sidebar}</div>
        </>
      )}
    </>
  );
}

function SidebarLink({
  label,
  icon,
  selected,
  nested = false,
  onClick,
}: {
  label: string;
  icon: React.ReactNode;
  selected: boolean;
  nested?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`sidebar-link${selected ? " selected" : ""}${nested ? " nested" : ""}`}
      aria-current={selected ? "page" : undefined}
      onClick={onClick}
    >
      <span className="sidebar-link-icon" aria-hidden="true">
        {icon}
      </span>
      <span>{label}</span>
    </button>
  );
}
