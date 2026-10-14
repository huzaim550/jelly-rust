import { api } from "../api/client";
import type { BaseItem } from "../types";

/** Runtime ticks (100ns units) to `1h 23m` or `23m`. */
export function formatDuration(ticks?: number | null): string {
  if (!ticks || ticks <= 0) return "";
  const total = Math.floor(ticks / 10_000_000);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  if (hours) return `${hours}h ${minutes}m`;
  if (minutes) return `${minutes}m`;
  return `${total}s`;
}

/** Seconds to `1:01:01` or `1:30`. */
export function formatTime(seconds: number): string {
  const total = Math.floor(Math.max(0, seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return hours ? `${hours}:${pad(minutes)}:${pad(secs)}` : `${minutes}:${pad(secs)}`;
}

/** A short subtitle line: year, episode code, and runtime. */
export function itemSubtitle(item: BaseItem): string {
  const parts: string[] = [];
  if (item.ProductionYear) parts.push(String(item.ProductionYear));

  if (item.Type === "Episode") {
    const season = item.ParentIndexNumber;
    const episode = item.IndexNumber;
    if (episode != null) {
      const code = season != null ? `S${season}E${episode}` : `E${episode}`;
      parts.push(code.replace(/\bS(\d)\b/, "S0$1").replace(/\bE(\d)\b/, "E0$1"));
    }
  } else if (item.Type === "Season" && item.IndexNumber != null) {
    parts.push(`Season ${item.IndexNumber}`);
  }

  const runtime = formatDuration(item.RunTimeTicks);
  if (runtime) parts.push(runtime);
  return parts.join(" · ");
}

/** Fraction of an item that has been watched, or 0. */
export function resumeFraction(item: BaseItem): number {
  const position = item.UserData?.PlaybackPositionTicks ?? 0;
  const runtime = item.RunTimeTicks ?? 0;
  if (!runtime || position <= 0) return 0;
  // Past the 98% mark an item counts as finished, matching the server.
  return Math.min(1, position / runtime);
}

export function hasPoster(item: BaseItem): boolean {
  return Boolean(item.ImageTags?.Primary);
}

export function posterUrl(item: BaseItem, width = 400): string {
  return api.imageUrl(item.Id, "Primary", width);
}

/**
 * A poster tile for an item.
 *
 * Rendered as a button rather than a div so keyboard navigation and screen
 * readers work without extra wiring.
 */
export function ItemCard({
  item,
  onOpen,
  onPlay,
  showPlayOverlay = true,
}: {
  item: BaseItem;
  onOpen: () => void;
  onPlay: () => void;
  showPlayOverlay?: boolean;
}) {
  const progress = resumeFraction(item);
  const subtitle = itemSubtitle(item);

  return (
    <div className="card item">
      <button type="button" className="poster-button" onClick={onOpen}>
        {hasPoster(item) ? (
          <img src={posterUrl(item)} alt="" loading="lazy" />
        ) : (
          <div className="poster placeholder">{item.Type}</div>
        )}
        {progress > 0 && progress < 0.98 && (
          <div className="progress" style={{ width: `${progress * 100}%` }} />
        )}
      </button>

      {showPlayOverlay && (
        <button
          type="button"
          className="overlay-play"
          aria-label={`Play ${item.Name}`}
          onClick={(event) => {
            // The tile itself is also clickable; don't double-fire.
            event.stopPropagation();
            onPlay();
          }}
        >
          ▶
        </button>
      )}

      <button type="button" className="meta" onClick={onOpen}>
        <span className="title">{item.Name}</span>
        {subtitle && <span className="sub muted">{subtitle}</span>}
      </button>
    </div>
  );
}

/** A responsive grid of posters. */
export function PosterGrid({
  items,
  onOpen,
  onPlay,
  emptyText = "Nothing here yet.",
}: {
  items: BaseItem[];
  onOpen: (item: BaseItem) => void;
  onPlay: (item: BaseItem) => void;
  emptyText?: string;
}) {
  if (items.length === 0) {
    return <p className="muted empty">{emptyText}</p>;
  }

  return (
    <div className="grid posters">
      {items.map((item) => (
        <ItemCard
          key={item.Id}
          item={item}
          onOpen={() => onOpen(item)}
          onPlay={() => onPlay(item)}
        />
      ))}
    </div>
  );
}