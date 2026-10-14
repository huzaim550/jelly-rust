import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { PosterGrid } from "../components/ItemCard";
import type { BaseItem } from "../types";
import type { Route } from "../router";

/**
 * Landing screen: continue watching, libraries, and next up.
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

  const [views, setViews] = useState<BaseItem[]>([]);
  const [resume, setResume] = useState<BaseItem[]>([]);
  const [nextUp, setNextUp] = useState<BaseItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!userId) return;
    let cancelled = false;

    (async () => {
      setLoading(true);
      try {
        // Fetch independently so one slow endpoint does not blank the page.
        const [v, r, n] = await Promise.allSettled([
          api.views(userId),
          api.resume(userId),
          api.nextUp(userId),
        ]);
        if (cancelled) return;

        if (v.status === "fulfilled") setViews(v.value);
        else setError("Could not load libraries.");

        // The remaining two are optional; a failure just hides that section.
        if (r.status === "fulfilled") setResume(r.value.Items);
        if (n.status === "fulfilled") setNextUp(n.value.Items);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [userId]);

  const openItem = (item: BaseItem) => go({ name: "item", itemId: item.Id });
  const playItem = (item: BaseItem) => onPlay({ itemId: item.Id, title: item.Name });

  if (loading) {
    return <p className="muted loading">Loading…</p>;
  }

  return (
    <main>
      {error && <p className="error">{error}</p>}

      {resume.length > 0 && (
        <section>
          <h2>Continue watching</h2>
          <PosterGrid items={resume} onOpen={openItem} onPlay={playItem} />
        </section>
      )}

      <section>
        <h2>Libraries</h2>
        {views.length === 0 ? (
          <div className="empty">
            <p>No libraries yet.</p>
            <p className="muted">
              Register one from the command line:{" "}
              <code>jf-admin add-library /path/to/media movies</code>
            </p>
          </div>
        ) : (
          <div className="grid views">
            {views.map((view) => (
              <button
                key={view.Id}
                type="button"
                className="card view"
                onClick={() => go({ name: "library", viewId: view.Id })}
              >
                <span className="view-icon" aria-hidden="true">
                  {iconFor(view.CollectionType)}
                </span>
                <span className="view-name">{view.Name}</span>
              </button>
            ))}
          </div>
        )}
      </section>

      {nextUp.length > 0 && (
        <section>
          <h2>Next up</h2>
          <PosterGrid items={nextUp} onOpen={openItem} onPlay={playItem} />
        </section>
      )}
    </main>
  );
}

function iconFor(collectionType?: string | null): string {
  switch ((collectionType ?? "").toLowerCase()) {
    case "movies":
      return "🎬";
    case "tvshows":
      return "📺";
    case "music":
      return "🎵";
    case "musicvideos":
      return "🎤";
    case "books":
      return "📚";
    case "photos":
      return "🖼️";
    default:
      return "📁";
  }
}