import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api, ApiError } from "../api/client";
import { PosterGrid } from "../components/ItemCard";
import type { BaseItem } from "../types";
import type { Route } from "../router";

/**
 * A grid of items, for either a library view or a search result.
 *
 * `viewId` is null for search, which queries across every library instead.
 */
export function Library({
  viewId,
  search,
  go,
  onPlay,
}: {
  viewId: string | null;
  search?: string;
  go: (route: Route, replace?: boolean) => void;
  onPlay: (item: { itemId: string; title: string }) => void;
}) {
  const { session } = useSession();
  const userId = session?.user.Id ?? "";

  const [items, setItems] = useState<BaseItem[]>([]);
  const [title, setTitle] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!userId) return;
    let cancelled = false;

    (async () => {
      setLoading(true);
      setError("");
      try {
        if (search) {
          const result = await api.items(userId, {
            searchTerm: search,
            recursive: true,
            limit: 200,
            fields: "Overview,ProductionYear,RunTimeTicks,UserData",
          });
          if (!cancelled) {
            setItems(result.Items);
            setTitle(`Results for “${search}”`);
          }
          return;
        }

        if (!viewId) return;

        const view = await api.item(userId, viewId);
        if (!cancelled) setTitle(view.Name);

        const result = await api.items(userId, {
          parentId: viewId,
          recursive: true,
          limit: 500,
          sortBy: "SortName",
          fields: "Overview,ProductionYear,RunTimeTicks,UserData",
        });
        if (!cancelled) setItems(result.Items);
      } catch (err) {
        if (cancelled) return;
        setError(err instanceof ApiError ? err.message : String(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [userId, viewId, search]);

  const openItem = (item: BaseItem) => go({ name: "item", itemId: item.Id });
  const playItem = (item: BaseItem) => onPlay({ itemId: item.Id, title: item.Name });

  return (
    <main>
      <div className="section-head">
        <h1>{title ?? (search ? "Search" : "Library")}</h1>
        {!loading && items.length > 0 && (
          <span className="muted small">{items.length} items</span>
        )}
      </div>

      {error && <p className="error">{error}</p>}
      {loading && <p className="muted loading">Loading…</p>}

      {!loading && !error && (
        <PosterGrid
          items={items}
          onOpen={openItem}
          onPlay={playItem}
          emptyText={search ? "No matches." : "This library is empty."}
        />
      )}
    </main>
  );
}