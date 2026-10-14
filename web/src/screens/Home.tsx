import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { itemSubtitle, PosterRail } from "../components/ItemCard";
import { libraryLabel, useLibraries } from "../library";
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

  const [views, setViews] = useState<BaseItem[]>([]);
  const [viewsError, setViewsError] = useState("");
  const [resume, setResume] = useState<BaseItem[]>([]);
  const [nextUp, setNextUp] = useState<BaseItem[]>([]);
  const [recent, setRecent] = useState<BaseItem[]>([]);
  const [catalogError, setCatalogError] = useState("");
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!userId) return;
    let cancelled = false;

    (async () => {
      // Independent requests so one slow endpoint does not blank the page.
      const [v, r, n, latest] = await Promise.allSettled([
        api.views(userId),
        api.resume(userId, 12),
        api.nextUp(userId, 12),
        api.latest(userId, 16),
      ]);
      if (cancelled) return;

      if (v.status === "fulfilled") {
        setViews(v.value);
        setViewsError("");
      } else {
        setViewsError(v.reason instanceof Error ? v.reason.message : String(v.reason));
      }
      if (r.status === "fulfilled") setResume(r.value.Items);
      if (n.status === "fulfilled") setNextUp(n.value.Items);
      if (latest.status === "fulfilled") {
        setRecent(latest.value.Items);
        setCatalogError("");
      } else {
        setCatalogError(latest.reason instanceof Error ? latest.reason.message : String(latest.reason));
      }
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
    void Promise.all([api.resume(userId, 12), api.latest(userId, 16)])
      .then(([resumeResult, latestResult]) => {
        setResume(resumeResult.Items);
        setRecent(latestResult.Items);
        setCatalogError("");
      })
      .catch((err: unknown) => {
        setCatalogError(err instanceof Error ? err.message : String(err));
      });
  }, [scanning, userId]);

  const openItem = (item: BaseItem) => go({ name: "item", itemId: item.Id });
  const playItem = (item: BaseItem) => onPlay({ itemId: item.Id, title: item.Name });
  const featured = resume[0] ?? recent[0] ?? nextUp[0] ?? null;
  const featuredImageId = featured?.ParentBackdropItemId || featured?.Id;
  const featuredImageKind =
    featured && (featured.ParentBackdropItemId || featured.BackdropImageTags?.length)
      ? "Backdrop"
      : "Primary";
  const featuredImageUrl = featured?.ImageUrls?.Backdrop || featured?.ImageUrls?.Primary;
  const recentItems = recent;

  if (loading && folders.length === 0) {
    return <p className="muted loading">Loading…</p>;
  }

  return (
    <main className="home-page">
      {(error || viewsError || catalogError) && (
        <p className="error">{error || viewsError || catalogError}</p>
      )}

      {scanning ? (
        <ScanPanel folders={folders} statuses={statuses} />
      ) : (
        <>
          {featured && (
            <section className="featured">
              {featuredImageId && (
                <img
                  className="featured-image"
                  src={featuredImageUrl || api.imageUrl(featuredImageId, featuredImageKind, 1600)}
                  alt=""
                  fetchPriority="high"
                  onError={(event) => {
                    event.currentTarget.style.visibility = "hidden";
                  }}
                />
              )}
              <div className="featured-copy">
                <span className="featured-eyebrow">Featured for you</span>
                <h1>{featured.Name}</h1>
                <div className="featured-meta">
                  {featured.Type === "Episode" && featured.SeriesName && (
                    <span>{featured.SeriesName}</span>
                  )}
                  {itemSubtitle(featured) && <span>{itemSubtitle(featured)}</span>}
                </div>
                {featured.Overview && <p>{featured.Overview}</p>}
                <div className="featured-actions">
                  <button type="button" className="primary" onClick={() => playItem(featured)}>
                    <span aria-hidden="true">▶</span> Play
                  </button>
                  <button type="button" className="featured-secondary" onClick={() => openItem(featured)}>
                    More info
                  </button>
                </div>
              </div>
            </section>
          )}

          {views.length > 0 ? (
            <section className="home-libraries">
              <div className="catalog-heading">
                <div>
                  <span className="catalog-kicker">Your collection</span>
                  <h2>My Libraries</h2>
                </div>
              </div>
              <div className="grid views">
                {views.map((view) => {
                  const folder = folders.find(
                    (candidate) => candidate.Name.toLowerCase() === view.Name.toLowerCase(),
                  );
                  const status = folder ? statuses[folder.ItemId] : undefined;
                  return (
                    <button
                      key={view.Id}
                      type="button"
                      className="card view"
                      onClick={() => go({ name: "library", viewId: view.Id })}
                    >
                      <span className="view-icon" aria-hidden="true">
                        <svg viewBox="0 0 24 24">
                          <path d="M4 5.75h6l2 2h8v10.5H4z" />
                          <path d="M4.5 9.25h15" />
                        </svg>
                      </span>
                      <span className="view-copy">
                        <span className="view-name">{view.Name}</span>
                        <span className="muted small">{libraryLabel(view.CollectionType ?? null)}</span>
                        {status?.state === "Done" && status.detail?.startsWith("No supported media") && (
                          <span className="muted small">{status.detail}</span>
                        )}
                      </span>
                    </button>
                  );
                })}
              </div>
            </section>
          ) : (
            <section className="home-empty">
              <h2>Start building your library</h2>
              <p className="muted">
                No libraries yet. Add a media folder to see your movies and shows here.
              </p>
              <button type="button" className="primary" onClick={() => go({ name: "dashboard" })}>
                Open library settings
              </button>
            </section>
          )}
        </>
      )}

      {resume.length > 0 && (
        <PosterRail
          title="Continue Watching"
          kicker="Pick up where you left off"
          items={resume}
          onOpen={openItem}
          onPlay={playItem}
        />
      )}

      {nextUp.length > 0 && (
        <PosterRail
          title="Next Up"
          kicker="Keep your series going"
          items={nextUp}
          onOpen={openItem}
          onPlay={playItem}
        />
      )}

      {recentItems.length > 0 && (
        <PosterRail
          title="Recently Added"
          kicker="Fresh from your collection"
          items={recentItems}
          onOpen={openItem}
          onPlay={playItem}
        />
      )}
    </main>
  );
}

/** Replaces the tiles while a scan runs. */
function ScanPanel({
  folders,
  statuses,
}: {
  folders: Array<{ ItemId: string; Name: string }>;
  statuses: Record<
    string,
    { state: string; progress: number | null; processed?: number; detail?: string | null }
  >;
}) {
  return (
    <section>
      <h2>Scanning your libraries…</h2>
      <ul className="scan-list">
        {folders.map((folder) => {
          const status = statuses[folder.ItemId];
          const progress = status?.progress ?? 0;
          const percent = Math.round(progress * 100);
          return (
            <li key={folder.ItemId}>
              <span className="name">{folder.Name}</span>
              <div className="bar">
                <div className="fill" style={{ width: `${percent}%` }} />
              </div>
              <span className="muted small">
                {status?.state === "Scanning" ? `${percent}%` : (status?.state ?? "Waiting")}
              </span>
              {status?.state === "Scanning" && status.processed != null && (
                <span className="muted small scan-count">{status.processed} files</span>
              )}
              {status?.state === "Scanning" && status.detail && (
                // The server reports the file being committed right now.
                <span className="muted small scan-current" title={status.detail}>
                  {status.detail.split("/").pop()}
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
