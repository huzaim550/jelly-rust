import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { PosterGrid } from "../components/ItemCard";
import { libraryIcon, libraryLabel, useLibraries } from "../library";
import type { BaseItem } from "../types";
import type { Route } from "../router";

/**
 * Landing screen: library tiles, then continue watching and next up.
 *
 * Library tiles come from `/Library/VirtualFolders` rather than the user's
 * views, because that is the same source the admin screen edits. Showing the
 * views endpoint here meant a freshly-added library did not appear until the
 * user list caught up.
 *
 * While a scan runs the tiles are replaced by progress, so an empty dashboard
 * after adding a library reads as "working" rather than "broken".
 */
export function Home({
  go,
  onPlay,
}: {
  go: (route: Route, replace?: boolean) => void;
  onPlay: (item: { itemId: string; title: string }) => void;
}) {
  const { session } = useSession();
  const userId = session?.user.Id ?? "";
  const { folders, scanning, statuses, error } = useLibraries();

  const [resume, setResume] = useState<BaseItem[]>([]);
  const [nextUp, setNextUp] = useState<BaseItem[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!userId) return;
    let cancelled = false;

    (async () => {
      // Independent requests so one slow endpoint does not blank the page.
      const [r, n] = await Promise.allSettled([api.resume(userId, 12), api.nextUp(userId, 12)]);
      if (cancelled) return;

      if (r.status === "fulfilled") setResume(r.value.Items);
      if (n.status === "fulfilled") setNextUp(n.value.Items);
    })().finally(() => {
      if (!cancelled) setLoading(false);
    });

    return () => {
      cancelled = true;
    };
  }, [userId]);

  // A finished scan changes what is on the dashboard, so reload when the
  // scanning flag flips back to false.
  useEffect(() => {
    if (scanning || !userId) return;
    void api
      .resume(userId, 12)
      .then((r) => setResume(r.Items))
      .catch(() => {});
  }, [scanning, userId]);

  const openItem = (item: BaseItem) => go({ name: "item", itemId: item.Id });
  const playItem = (item: BaseItem) => onPlay({ itemId: item.Id, title: item.Name });

  if (loading && folders.length === 0) {
    return <p className="muted loading">Loading…</p>;
  }

  return (
    <main>
      {error && <p className="error">{error}</p>}

      {scanning ? (
        <ScanPanel folders={folders} statuses={statuses} />
      ) : (
        <section>
          <h2>My Libraries</h2>
          {folders.length === 0 ? (
            <div className="empty">
              <p>No libraries yet.</p>
              <p className="muted">
                Add one from{" "}
                <button type="button" className="link" onClick={() => go({ name: "dashboard" })}>
                  library settings
                </button>
                , or run <code>jf-admin add-library /path/to/media movies</code>.
              </p>
            </div>
          ) : (
            <div className="grid views">
              {folders.map((folder) => (
                <button
                  key={folder.ItemId}
                  type="button"
                  className="card view"
                  onClick={() => go({ name: "library", viewId: folder.ItemId })}
                >
                  <span className="view-icon" aria-hidden="true">
                    {libraryIcon(folder.CollectionType)}
                  </span>
                  <span className="view-name">{folder.Name}</span>
                  <span className="muted small">{libraryLabel(folder.CollectionType)}</span>
                </button>
              ))}
            </div>
          )}
        </section>
      )}

      {resume.length > 0 && (
        <section>
          <h2>Continue Watching</h2>
          <PosterGrid items={resume} onOpen={openItem} onPlay={playItem} />
        </section>
      )}

      {nextUp.length > 0 && (
        <section>
          <h2>Next Up</h2>
          <PosterGrid items={nextUp} onOpen={openItem} onPlay={playItem} />
        </section>
      )}

      {!loading && resume.length === 0 && nextUp.length === 0 && folders.length > 0 && (
        <section>
          <h2>Recently Added</h2>
          <RecentItems userId={userId} open={openItem} play={playItem} />
        </section>
      )}
    </main>
  );
}

/** Recently added items, so a fresh library is not an empty dashboard. */
function RecentItems({
  userId,
  open,
  play,
}: {
  userId: string;
  open: (item: BaseItem) => void;
  play: (item: BaseItem) => void;
}) {
  const [items, setItems] = useState<BaseItem[]>([]);

  useEffect(() => {
    let cancelled = false;
    void api
      .latest(userId, 12)
      .then((r) => {
        if (!cancelled) setItems(r.Items);
      })
      .catch(() => {
        // A missing "latest" endpoint should not break the dashboard.
      });
    return () => {
      cancelled = true;
    };
  }, [userId]);

  if (items.length === 0) return null;
  return <PosterGrid items={items} onOpen={open} onPlay={play} />;
}

/** Replaces the tiles while a scan runs. */
function ScanPanel({
  folders,
  statuses,
}: {
  folders: Array<{ ItemId: string; Name: string }>;
  statuses: Record<string, { state: string; progress: number | null }>;
}) {
  return (
    <section>
      <h2>Scanning your libraries…</h2>
      <ul className="scan-list">
        {folders.map((folder) => {
          const status = statuses[folder.ItemId];
          const progress = status?.progress ?? 0;
          return (
            <li key={folder.ItemId}>
              <span className="name">{folder.Name}</span>
              <div className="bar">
                <div className="fill" style={{ width: `${Math.round(progress * 100)}%` }} />
              </div>
              <span className="muted small">
                {status?.state === "Scanning"
                  ? `${Math.round(progress * 100)}%`
                  : (status?.state ?? "Waiting")}
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
