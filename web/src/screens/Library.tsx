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
  const [reload, setReload] = useState(0);

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

        const [views, result] = await Promise.all([
          api.views(userId),
          api.items(userId, {
            parentId: viewId,
            recursive: true,
            limit: 500,
            sortBy: "SortName",
            fields: "Overview,ProductionYear,RunTimeTicks,UserData",
          }),
        ]);
        const view = views.find((candidate) => candidate.Id === viewId);
        if (!view) throw new Error("Library not found.");
        if (!cancelled) setTitle(view.Name);
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
  }, [userId, viewId, search, reload]);

  useEffect(() => {
    if (!session?.user.Policy.IsAdministrator) return;
    let disposed = false;
    let checking = false;
    let scanObserved = false;

    const checkScan = async () => {
      if (checking) return;
      checking = true;
      try {
        const status = await api.scanStatus();
        if (status.scanning) {
          scanObserved = true;
        } else if (scanObserved && !disposed) {
          scanObserved = false;
          setReload((value) => value + 1);
        }
      } catch {
        // Scan status is admin-only and may be temporarily unavailable.
      } finally {
        checking = false;
      }
    };

    void checkScan();
    const timer = window.setInterval(() => void checkScan(), 1500);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [session?.user.Policy.IsAdministrator]);

  const openItem = (item: BaseItem) => go({ name: "item", itemId: item.Id });
  const playItem = (item: BaseItem) => onPlay({ itemId: item.Id, title: item.Name });

  return (
    <main>
      <div className="section-head">
        {viewId && (
          <button
            type="button"
            className="link library-back"
            onClick={() => go({ name: "home" })}
          >
            <span aria-hidden="true">←</span>
            <span>Libraries</span>
          </button>
        )}
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