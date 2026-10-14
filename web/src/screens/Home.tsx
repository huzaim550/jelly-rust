import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { itemSubtitle, initials, PosterRail } from "../components/ItemCard";
import { libraryLabel, useLibraries } from "../library";
import type { BaseItem } from "../types";
import type { Route } from "../router";

/**
 * Landing screen: one continuous page — a hero banner, then rows of posters.
 *
 * This is a single scroll surface on purpose. It was briefly split into a
 * scrolling browser column beside a sticky hero panel, but two independently
 * scrolling halves meant the hero was always pinned while the rows moved under
 * it, which read as two apps rather than one.
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
  const [slideIndexRaw, setSlideIndexRaw] = useState(0);

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

  // The banner rotates through the titles that actually have artwork; anything
  // without an image would render as an empty black panel.
  const slides = [...recent, ...resume, ...nextUp]
    .filter(
      (item) =>
        item.ImageUrls?.Backdrop ||
        item.ImageUrls?.Primary ||
        item.ParentBackdropItemId ||
        item.BackdropImageTags?.length,
    )
    .filter((item, index, all) => all.findIndex((other) => other.Id === item.Id) === index)
    .slice(0, 6);

  // Keep the index in range if a refresh returns fewer featured titles.
  const slideIndex = Math.min(slideIndexRaw, Math.max(0, slides.length - 1));
  const featured = slides[slideIndex] ?? resume[0] ?? nextUp[0] ?? recent[0] ?? null;
  const featuredImageId = featured?.ParentBackdropItemId || featured?.Id;
  const featuredImageKind =
    featured && (featured.ParentBackdropItemId || featured.BackdropImageTags?.length)
      ? "Backdrop"
      : "Primary";
  const featuredImageUrl =
    featured?.ImageUrls?.Backdrop ||
    featured?.ImageUrls?.Primary ||
    (featuredImageId ? api.imageUrl(featuredImageId, featuredImageKind, 1600) : undefined);
  // Nameless people entries do occur on the wire, and rendering one as an empty
  // circle (or crashing on `.split`) is worse than leaving it out.
  const cast = (featured?.People ?? []).filter(
    (person) => Boolean(person.Name) && (!person.Type || person.Type.toLowerCase() === "actor"),
  );

  function stepSlide(direction: -1 | 1) {
    if (slides.length === 0) return;
    setSlideIndexRaw((slideIndex + direction + slides.length) % slides.length);
  }

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
        <div className="home-layout">
          {featured && (
            <section className="hero-banner" aria-label={`Featured: ${featured.Name}`}>
              <div className="hero-banner-art">
                {featuredImageUrl && (
                  <img
                    src={featuredImageUrl}
                    alt=""
                    fetchPriority="high"
                    onError={(event) => {
                      event.currentTarget.style.visibility = "hidden";
                    }}
                  />
                )}
                <span className="hero-banner-tag">Featured for you</span>

                {slides.length > 1 && (
                  <>
                    <button
                      type="button"
                      className="hero-slide-arrow prev"
                      aria-label="Previous featured title"
                      onClick={() => stepSlide(-1)}
                    >
                      ‹
                    </button>
                    <button
                      type="button"
                      className="hero-slide-arrow next"
                      aria-label="Next featured title"
                      onClick={() => stepSlide(1)}
                    >
                      ›
                    </button>
                    <div className="hero-slide-dots">
                      {slides.map((slide, index) => (
                        <button
                          key={slide.Id}
                          type="button"
                          className="hero-slide-dot"
                          aria-label={`Feature ${slide.Name}`}
                          aria-current={index === slideIndex}
                          onClick={() => setSlideIndexRaw(index)}
                        />
                      ))}
                    </div>
                  </>
                )}
              </div>

              <div className="hero-banner-copy">
                <h1>{featured.Name}</h1>
                {featured.Type === "Episode" && featured.SeriesName && (
                  <p className="hero-banner-series">{featured.SeriesName}</p>
                )}
                <div className="hero-banner-meta">
                  {itemSubtitle(featured) && <span>{itemSubtitle(featured)}</span>}
                  {featured.CommunityRating != null && featured.CommunityRating > 0 && (
                    <span className="hero-banner-rating">★ {featured.CommunityRating.toFixed(1)}</span>
                  )}
                  {featured.OfficialRating && (
                    <span className="hero-banner-cert">{featured.OfficialRating}</span>
                  )}
                </div>

                {featured.Overview && (
                  <p className="hero-banner-overview">{featured.Overview}</p>
                )}

                <div className="hero-banner-actions">
                  <button
                    type="button"
                    className="home-watch-button"
                    onClick={() => playItem(featured)}
                  >
                    <span aria-hidden="true">▶</span> Watch Now
                  </button>
                  <button
                    type="button"
                    className="home-info-button"
                    onClick={() => openItem(featured)}
                  >
                    More info
                  </button>
                </div>

                {cast.length > 0 && (
                  <div className="hero-banner-cast">
                    <h2>Cast</h2>
                    <div className="home-cast-list">
                      {cast.slice(0, 7).map((person) => (
                        <div className="home-cast-member" key={person.Id}>
                          <span className="home-cast-avatar" aria-hidden="true">
                            {initials(person.Name)}
                          </span>
                          <span className="name">{person.Name}</span>
                        </div>
                      ))}
                      {cast.length > 7 && (
                        <button
                          type="button"
                          className="home-cast-more"
                          onClick={() => openItem(featured)}
                        >
                          View all
                        </button>
                      )}
                    </div>
                  </div>
                )}
              </div>
            </section>
          )}

          <div className="home-rows" aria-label="Browse your library">
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
            {recent.length > 0 && (
              <PosterRail
                title="Recently Added"
                kicker="Fresh from your collection"
                items={recent}
                onOpen={openItem}
                onPlay={playItem}
              />
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
                          <span className="muted small">
                            {libraryLabel(view.CollectionType ?? null)}
                          </span>
                          {status?.state === "Done" &&
                            status.detail?.startsWith("No supported media") && (
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
                <button
                  type="button"
                  className="primary"
                  onClick={() => go({ name: "dashboard" })}
                >
                  Open library settings
                </button>
              </section>
            )}
          </div>
        </div>
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