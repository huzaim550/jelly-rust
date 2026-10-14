import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api, ApiError } from "../api/client";
import { PosterGrid, formatDuration, itemSubtitle } from "../components/ItemCard";
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

  return (
    <main>
      <button type="button" className="link back" onClick={() => history.back()}>
        ← Back
      </button>

      <div className="detail">
        <div className="detail-art">
          {item.ImageTags?.Primary ? (
            <img src={api.imageUrl(item.Id, "Primary", 500)} alt="" />
          ) : (
            <div className="poster placeholder">{item.Type}</div>
          )}
        </div>

        <div className="detail-body">
          <h1>{item.Name}</h1>
          <p className="muted">{itemSubtitle(item)}</p>

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

          <dl className="facts">
            {item.RunTimeTicks ? (
              <>
                <dt>Runtime</dt>
                <dd>{formatDuration(item.RunTimeTicks)}</dd>
              </>
            ) : null}
            {item.CommunityRating ? (
              <>
                <dt>Rating</dt>
                <dd>{item.CommunityRating.toFixed(1)}</dd>
              </>
            ) : null}
            {item.OfficialRating ? (
              <>
                <dt>Certificate</dt>
                <dd>{item.OfficialRating}</dd>
              </>
            ) : null}
          </dl>

          <div className="actions">
            <button
              type="button"
              className="primary"
              onClick={() => onPlay({ itemId: item.Id, title: item.Name })}
            >
              Play
            </button>
            <button
              type="button"
              className="ghost"
              onClick={() => void toggleFavorite()}
              disabled={favoriteBusy}
            >
              {isFavorite ? "♥ Remove from favourites" : "♡ Add to favourites"}
            </button>
          </div>
        </div>
      </div>

      {children.length > 0 && (
        <section>
          <h2>{item.Type === "Season" ? "Episodes" : "Seasons"}</h2>
          <PosterGrid
            items={children}
            onOpen={(child) => go({ name: "item", itemId: child.Id })}
            onPlay={(child) => onPlay({ itemId: child.Id, title: child.Name })}
            emptyText=""
          />
        </section>
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