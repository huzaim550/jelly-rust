/**
 * Theme manager.
 *
 * Mirrors Jellyfin's approach: the active scheme is a `data-theme` attribute on
 * `<html>` and the palettes live as CSS variables (`theme.css`), so switching
 * is one attribute write rather than re-rendering the tree. Jellyfin wires this
 * through an event so other components stay in step (its `ThemeStorageManager`
 * subscribes to a THEME_CHANGE event); we do the same with a DOM event, which
 * also lets the inline bootstrap script in index.html apply a stored theme
 * before first paint.
 */

import { useCallback, useEffect, useState } from "react";

export interface ThemeDefinition {
  /** Matches the `[data-theme="…"]` selector in theme.css. */
  id: string;
  label: string;
  /** Drives the `<meta name="color-scheme">` hint. */
  dark: boolean;
  /** A two-colour preview: the user-facing way to tell schemes apart. */
  swatch: { primary: string; background: string };
}

/**
 * The schemes Jellyfin ships, in its order, plus this client's original palette.
 * Values are only used for previews; the palettes themselves live in theme.css.
 */
export const THEMES: ThemeDefinition[] = [
  { id: "dark", label: "Jellyfin Dark", dark: true, swatch: { primary: "#00a4dc", background: "#101010" } },
  { id: "light", label: "Jellyfin Light", dark: false, swatch: { primary: "#00a4dc", background: "#f2f2f2" } },
  { id: "appletv", label: "Apple TV", dark: false, swatch: { primary: "#00a4dc", background: "#d5e9f2" } },
  { id: "blueradiance", label: "Blue Radiance", dark: true, swatch: { primary: "#00a4dc", background: "#033361" } },
  { id: "purplehaze", label: "Purple Haze", dark: true, swatch: { primary: "#48c3c8", background: "#230c33" } },
  { id: "wmc", label: "Windows Media Center", dark: true, swatch: { primary: "#00a4dc", background: "#0f3562" } },
  { id: "cinema", label: "Cinema", dark: true, swatch: { primary: "#e50914", background: "#0b0b0d" } },
];

/** Jellyfin's default scheme, and ours. */
export const DEFAULT_THEME = "dark";

const STORAGE_KEY = "jf.theme";
const THEME_EVENT = "jf:theme";

function isKnownTheme(id: string | null | undefined): id is string {
  return typeof id === "string" && THEMES.some((theme) => theme.id === id);
}

/** Read the stored theme, falling back to the default. Never throws. */
export function getStoredTheme(): string {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    return isKnownTheme(stored) ? stored : DEFAULT_THEME;
  } catch {
    // Private-mode browsers throw on access; an in-memory default is fine.
    return DEFAULT_THEME;
  }
}

/** Set a `<meta>` by name, creating it if the document lacks one. */
function setMeta(name: string, content: string): void {
  let meta = document.head.querySelector<HTMLMetaElement>(`meta[name="${name}"]`);
  if (!meta) {
    meta = document.createElement("meta");
    meta.name = name;
    document.head.appendChild(meta);
  }
  meta.content = content;
}

/** Write the palette to the document without persisting (used at boot). */
export function applyTheme(id: string): void {
  const theme = THEMES.find((candidate) => candidate.id === id) ?? THEMES[0]!;
  document.documentElement.dataset.theme = theme.id;
  setMeta("color-scheme", theme.dark ? "dark" : "light");
  setMeta("theme-color", theme.swatch.background);
}

/** Persist and apply a theme, then notify subscribers. */
export function setTheme(id: string): void {
  const theme = isKnownTheme(id) ? id : DEFAULT_THEME;
  try {
    window.localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // Not persisting is acceptable; the theme still applies for this session.
  }
  applyTheme(theme);
  window.dispatchEvent(new CustomEvent<string>(THEME_EVENT, { detail: theme }));
}

/** Run `handler` whenever the theme changes. Returns an unsubscribe function. */
export function subscribeTheme(handler: (id: string) => void): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<string>).detail;
    handler(isKnownTheme(detail) ? detail : getStoredTheme());
  };
  window.addEventListener(THEME_EVENT, listener);
  return () => window.removeEventListener(THEME_EVENT, listener);
}

/** Current theme plus a setter, kept in sync across the app. */
export function useTheme(): [string, (id: string) => void] {
  const [theme, setThemeState] = useState(getStoredTheme);

  useEffect(() => {
    // Re-assert on mount: a component that renders before the module-level
    // bootstrap (a test harness, or a client that mounted <App> directly) still
    // ends up with the stored palette on <html>.
    applyTheme(getStoredTheme());
    return subscribeTheme(setThemeState);
  }, []);

  const update = useCallback((id: string) => setTheme(id), []);
  return [theme, update];
}
