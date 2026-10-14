/**
 * A tiny router.
 *
 * The app has a handful of screens, so a dependency-free history-API router is
 * less machinery than pulling in a routing library — and it keeps shareable
 * URLs and the back button working, which is what actually matters here.
 */

import { useCallback, useEffect, useState } from "react";

/** Route patterns use `:name` for parameters, e.g. `/item/:id`. */
export type Route =
  | { name: "home" }
  | { name: "dashboard" }
  | { name: "api-keys" }
  | { name: "library"; viewId: string }
  | { name: "item"; itemId: string }
  | { name: "search"; term: string };

export function parseLocation(pathname: string, search: string): Route {
  const term = new URLSearchParams(search).get("q") ?? "";

  const segments = pathname.split("/").filter(Boolean);

  if (segments.length === 0) return { name: "home" };
  if (segments[0] === "library" && segments[1]) return { name: "library", viewId: segments[1] };
  if (segments[0] === "item" && segments[1]) return { name: "item", itemId: segments[1] };
  if (segments[0] === "dashboard") return { name: "dashboard" };
  if (segments[0] === "settings" && segments[1] === "api-keys") {
    return { name: "api-keys" };
  }
  if (segments[0] === "search") return { name: "search", term };
  return { name: "home" };
}

export function hrefFor(route: Route): string {
  switch (route.name) {
    case "home":
      return "/";
    case "dashboard":
      return "/dashboard";
    case "api-keys":
      return "/settings/api-keys";
    case "library":
      return `/library/${route.viewId}`;
    case "item":
      return `/item/${route.itemId}`;
    case "search":
      return `/search?q=${encodeURIComponent(route.term)}`;
  }
}

export function navigate(route: Route, replace = false): void {
  const href = hrefFor(route);
  if (replace) window.history.replaceState({}, "", href);
  else window.history.pushState({}, "", href);
  // `useRoute` subscribes to this custom event rather than `popstate`:
  // `replaceState` does not fire `popstate`, so relying on it left in-app
  // navigation updating the URL without updating the screen.
  window.dispatchEvent(new Event("jf:navigate"));
}

/**
 * Subscribe to location changes.
 *
 * Listens for both `popstate` (back/forward) and the custom event fired by
 * `navigate`, so in-app navigation and history navigation behave identically.
 */
export function useRoute(): [Route, (route: Route, replace?: boolean) => void] {
  const read = useCallback(
    () => parseLocation(window.location.pathname, window.location.search),
    [],
  );
  const [route, setRoute] = useState<Route>(read);

  useEffect(() => {
    const update = () => setRoute(read());
    window.addEventListener("popstate", update);
    window.addEventListener("jf:navigate", update);
    return () => {
      window.removeEventListener("popstate", update);
      window.removeEventListener("jf:navigate", update);
    };
  }, [read]);

  const go = useCallback((next: Route, replace = false) => {
    navigate(next, replace);
  }, []);

  return [route, go];
}