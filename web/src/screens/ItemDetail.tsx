import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api, ApiError } from "../api/client";
import { PosterRail, itemSubtitle } from "../components/ItemCard";
import type { BaseItem } from "../types";
import type { Route } from "../router";

/**
 * Item detail, plus the next level down for series (seasons) or a single
 * season (episodes).
 */
export function ItemDetail({
  itemId,
  go,
  onPlay,
}: {
  itemId: string;
  go: (route: Route, replace?: boolean) => void;
  onPlay: (item: { itemId: string; title: string }) => void;
}) {
  const { session } = useSession();
  const userId = session?.user.Id ?? "";

  const [item, setItem] = useState<BaseItem | null>(null);
  const [children, setChildren] = useState<BaseItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [favoriteBusy, setFavoriteBusy] = useState(false);

  useEffect(() => {
    if (!userId) return;
    let cancelled = false;

    (async () => {
      setLoading(true);
      setError("");
      try {
        const detail = await api.item(userId, itemId);
        if (cancelled) return;
        setItem(detail);
        document.title = `${detail.Name} — jellyfin-clone`;

        // Only fetch children when there is a level below to show.
        if (detail.Type === "Series") {
          const seasons = await api.seasons(detail.Id);
          if (!cancelled) setChildren(seasons.Items);
        } else if (detail.Type === "Season" && detail.SeriesId) {
          const episodes = await api.episodes(detail.SeriesId, detail.Id);
          if (!cancelled) setChildren(episodes.Items);
        }
      } catch (err) {
        if (!cancelled) setError(err instanceof ApiError ? err.message : String(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
      document.title = "jellyfin-clone";
    };
  }, [userId, itemId]);

  async function toggleFavorite() {
    if (!item || favoriteBusy) return;
    setFavoriteBusy(true);
    const next = !item.UserData?.IsFavorite;
    try {
      await api.setFavorite(item.Id, next);
      setItem({ ...item, UserData: { ...(item.UserData ?? emptyUserData), IsFavorite: next } });
    } catch {
      // Leave the UI as-is; the next reload shows the real value.
    } finally {
      setFavoriteBusy(false);
    }
  }

  if (loading) return <p className="muted loading">Loading…</p>;
  if (error) return <p className="error">{error}</p>;
  if (!item) return null;

  const isFavorite = item.UserData?.IsFavorite ?? false;
  const backdropItemId = item.ParentBackdropItemId || item.Id;
  const seriesId = item.SeriesId;
  const backdropUrl = item.ImageUrls?.Backdrop || item.ImageUrls?.Primary;
  const hasBackdrop = Boolean(backdropUrl || item.ParentBackdropItemId || item.BackdropImageTags?.length);

  return (
    <main className="item-detail-page">
      <section className={`detail-hero${hasBackdrop ? " has-backdrop" : ""}`}>
        {hasBackdrop && (
          <img
            className="detail-backdrop"
            src={backdropUrl || api.imageUrl(backdropItemId, "Backdrop", 1600)}
            alt=""
            fetchPriority="high"
            onError={(event) => {
              event.currentTarget.style.display = "none";
            }}
          />
        )}
        <div className="detail-hero-content">
          <button type="button" className="detail-back link" onClick={() => history.back()}>
            ← Back
          </button>

          <div className="detail">
            <div className="detail-art">
              {item.ImageTags?.Primary || item.ImageUrls?.Primary ? (
                <img
                  src={item.ImageUrls?.Primary || api.imageUrl(item.Id, "Primary", 600)}
                  alt={`${item.Name} poster`}
                />
              ) : (
                <div className="poster placeholder">{item.Type}</div>
              )}
            </div>

            <div className="detail-body">
              {item.Type === "Episode" && seriesId && (
                <button
                  type="button"
                  className="detail-series-link"
                  onClick={() => go({ name: "item", itemId: seriesId })}
                >
                  {item.SeriesName || "View series"}
                </button>
              )}
              <span className="detail-type">{item.Type}</span>
              <h1>{item.Name}</h1>

              <div className="detail-meta">
                {itemSubtitle(item) && <span>{itemSubtitle(item)}</span>}
                {item.CommunityRating != null && item.CommunityRating > 0 && (
                  <span className="detail-rating" aria-label={`Rating ${item.CommunityRating.toFixed(1)} out of 10`}>
                    <span aria-hidden="true">★</span> {item.CommunityRating.toFixed(1)}
                  </span>
                )}
                {item.OfficialRating && <span className="detail-cert">{item.OfficialRating}</span>}
              </div>

              {item.Overview && <p className="overview">{item.Overview}</p>}

              {item.Genres && item.Genres.length > 0 && (
                <div className="genres">
                  {item.Genres.map((genre) => (
                    <span key={genre} className="chip">
                      {genre}
                    </span>
                  ))}
                </div>
              )}

              <div className="actions detail-actions">
                <button
                  type="button"
                  className="primary detail-play"
                  onClick={() => onPlay({ itemId: item.Id, title: item.Name })}
                >
                  <span aria-hidden="true">▶</span> Play
                </button>
                <button
                  type="button"
                  className={`ghost detail-favorite${isFavorite ? " is-favorite" : ""}`}
                  onClick={() => void toggleFavorite()}
                  disabled={favoriteBusy}
                  aria-pressed={isFavorite}
                >
                  <span aria-hidden="true">{isFavorite ? "♥" : "♡"}</span>
                  {isFavorite ? "In favourites" : "Add to favourites"}
                </button>
              </div>
            </div>
          </div>
        </div>
      </section>

      {children.length > 0 && (
        <PosterRail
          title={item.Type === "Season" ? "Episodes" : "Seasons"}
          kicker={item.Name}
          items={children}
          onOpen={(child) => go({ name: "item", itemId: child.Id })}
          onPlay={(child) => onPlay({ itemId: child.Id, title: child.Name })}
        />
      )}
    </main>
  );
}

const emptyUserData = {
  PlaybackPositionTicks: 0,
  PlayCount: 0,
  IsFavorite: false,
  Played: false,
  UnplayedItemCount: 0,
};