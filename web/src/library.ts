import { useCallback, useEffect, useState } from "react";
import { api } from "./api/client";
import type { Route } from "./router";

/** Library kinds Jellyfin supports, matching its `CollectionTypeOptions`. */
export const LIBRARY_TYPES = [
  { value: "movies", label: "Movies", icon: "🎬", hint: "Feature films" },
  { value: "tvshows", label: "TV Shows", icon: "📺", hint: "Series and episodes" },
  { value: "music", label: "Music", icon: "🎵", hint: "Albums and tracks" },
  { value: "books", label: "Books", icon: "📚", hint: "Audiobooks and ebooks" },
  { value: "photos", label: "Photos", icon: "🖼️", hint: "Photo libraries" },
] as const;

export type LibraryType = (typeof LIBRARY_TYPES)[number]["value"];

/** Wire shape of Jellyfin's `VirtualFolderInfo` (PascalCase). */
export interface VirtualFolder {
  Name: string;
  Locations: string[];
  CollectionType: string | null;
  ItemId: string;
  RefreshProgress: number | null;
  RefreshStatus: string | null;
}

export interface ScanStatus {
  state: string;
  progress: number | null;
  processed: number;
  detail: string | null;
}

export function libraryLabel(collectionType: string | null): string {
  const found = LIBRARY_TYPES.find((t) => t.value === collectionType?.toLowerCase());
  return found?.label ?? "Mixed";
}

export function libraryIcon(collectionType: string | null): string {
  const found = LIBRARY_TYPES.find((t) => t.value === collectionType?.toLowerCase());
  return found?.icon ?? "📁";
}

/**
 * Library and scan state.
 *
 * `/Library/VirtualFolders` and `/Library/ScanStatus` are administrator-only on
 * the server, so `enabled` must be false for a normal member: otherwise every
 * visit to the home screen fires two requests that 403 and surfaces
 * "Managing libraries requires administrator access." at the top of a page the
 * member is perfectly entitled to see.
 */
export function useLibraries(enabled = true) {
  const [folders, setFolders] = useState<VirtualFolder[]>([]);
  const [scanning, setScanning] = useState(false);
  const [statuses, setStatuses] = useState<Record<string, ScanStatus>>({});
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    if (!enabled) return;
    try {
      setFolders(await api.virtualFolders());
      setError("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [enabled]);

  const pollStatus = useCallback(async () => {
    if (!enabled) return;
    try {
      const status = await api.scanStatus();
      setScanning(status.scanning);
      setStatuses(status.libraries ?? {});
    } catch {
      // A transient failure here must not break the page; the next poll retries.
    }
  }, [enabled]);

  useEffect(() => {
    void refresh();
    void pollStatus();
  }, [refresh, pollStatus]);

  // While a scan is running, poll for progress and follow the event stream for
  // completion. Polling stops as soon as the scan ends so an idle server is not
  // making requests forever.
  useEffect(() => {
    if (!scanning) return;
    const timer = window.setInterval(() => void pollStatus(), 1500);
    return () => window.clearInterval(timer);
  }, [scanning, pollStatus]);

  const add = useCallback(
    async (name: string, collectionType: LibraryType, path: string) => {
      await api.addVirtualFolder(name, collectionType, path);
      await Promise.all([refresh(), pollStatus()]);
    },
    [pollStatus, refresh],
  );

  const remove = useCallback(
    async (name: string) => {
      await api.deleteVirtualFolder(name);
      await refresh();
    },
    [refresh],
  );

  const rescan = useCallback(async () => {
    await api.refreshLibrary();
    await pollStatus();
  }, [pollStatus]);

  return { folders, scanning, statuses, error, refresh, add, remove, rescan, pollStatus };
}

/** Navigate helper shared by the wizard screens. */
export function useWizardNavigation(go: (route: Route, replace?: boolean) => void) {
  return useCallback(
    (route: Route, replace = true) => {
      go(route, replace);
    },
    [go],
  );
}