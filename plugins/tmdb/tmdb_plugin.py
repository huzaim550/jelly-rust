"""TMDB metadata provider.

The reference implementation of a metadata plugin: everything Jellyfin-clone
needs from a network provider, and nothing it does not.

Set your API key from the dashboard, or via a ``tmdb_api_key`` environment
variable for unattended setups. TMDB uses separate "v3 API key" and
"v4 read access token" credentials; this plugin accepts either and sends the
token as a Bearer header when it looks like one.

Configuration lives in the plugin's own namespace, so it is invisible to other
plugins and survives restarts.
"""

from __future__ import annotations

import os
import sys

import jellyplug
from jellyplug import ImageKind, MetadataImage, MetadataPerson, MetadataResult, PersonKind
from jellyplug.http import HttpError, fetch_json

API_BASE = "https://api.themoviedb.org/3"
IMAGE_BASE = "https://image.tmdb.org/t/p"

# TMDB genre ids that map cleanly onto Jellyfin's genres. Unknown ids are
# dropped rather than emitted as numbers.
GENRE_NAMES = {
    28: "Action",
    12: "Adventure",
    16: "Animation",
    35: "Comedy",
    80: "Crime",
    99: "Documentary",
    18: "Drama",
    10751: "Family",
    14: "Fantasy",
    36: "History",
    27: "Horror",
    10402: "Music",
    9648: "Mystery",
    10749: "Romance",
    878: "Science Fiction",
    10770: "TV Movie",
    53: "Thriller",
    10752: "War",
    37: "Western",
}

# A single hook is deadline-bounded by the host; keep our own budget tighter so
# a slow TMDB cannot consume the host's whole allowance.
REQUEST_TIMEOUT = 8.0

# How often to re-fetch metadata for an item that already has it. Metadata
# changes rarely, so re-querying on every scan is wasted requests.
REFRESH_AFTER_DAYS = 30


def setup() -> None:
    """Called once at load time. Must not block."""
    if not api_key():
        jellyplug.log(
            "warn",
            "TMDB plugin loaded without an API key; set one in the dashboard "
            "or TMDB_API_KEY to enable metadata fetching.",
        )
    jellyplug.require_capability("network")


def api_key() -> str | None:
    """Resolve credentials from config, then the environment."""
    try:
        stored = config.get("api_key")
    except Exception:  # noqa: BLE001 - standalone execution has no host
        stored = None
    if stored:
        return stored
    return os.environ.get("TMDB_API_KEY") or os.environ.get("TMDB_API_TOKEN")


def headers() -> dict[str, str]:
    """Build auth headers.

    A v4 read token is a long opaque JWT and must go in ``Authorization``.
    A v3 API key is a short hex string and uses the ``api_key`` query param.
    """
    key = api_key()
    if not key:
        return {}
    if len(key) > 40 or key.count(".") == 2:
        return {"Authorization": f"Bearer {key}"}
    return {"api_key": key}


def fetch_metadata(query_json: str) -> str | None:
    """Resolve one item.

    Returns a JSON string, or ``None`` when this provider has nothing, so the
    host falls through to the next provider in the chain.
    """
    query = jellyplug.parse_query(query_json)
    if not query.title.strip():
        return None

    if not api_key():
        jellyplug.log("debug", f"skipping {query.title!r}: no TMDB API key")
        return None

    try:
        if query.kind == "Episode":
            return jellyplug.dumps(_fetch_episode(query))
        if query.kind == "Series":
            return jellyplug.dumps(_fetch_series(query))
        return jellyplug.dumps(_fetch_movie(query))
    except HttpError as exc:
        if exc.status == 401:
            jellyplug.log("error", "TMDB rejected the API key; check the dashboard settings")
        elif exc.status == 404:
            jellyplug.log("debug", f"TMDB has no match for {query.title!r}")
        elif exc.status == 429:
            jellyplug.log("warn", "TMDB rate limit reached; skipping this item")
        else:
            jellyplug.log("warn", f"TMDB request failed for {query.title!r}: {exc}")
        return None
    except TimeoutError:
        jellyplug.log("warn", f"TMDB timed out for {query.title!r}")
        return None
    except Exception as exc:  # noqa: BLE001 - a plugin must never crash the host
        jellyplug.log("error", f"TMDB plugin error for {query.title!r}: {exc!r}")
        return None


# --------------------------------------------------------------------------
# Lookups
# --------------------------------------------------------------------------


def _fetch_movie(query: jellyplug.MetadataQuery) -> MetadataResult:
    tmdb_id = query.known_id("tmdb")
    data = _get(f"{API_BASE}/movie/{tmdb_id}" if tmdb_id else _search_movie_url(query))
    return _to_result(data, external_provider="tmdb", external_key="id", external_id=data.get("id"))


def _fetch_series(query: jellyplug.MetadataQuery) -> MetadataResult:
    tmdb_id = query.known_id("tmdb")
    data = _get(f"{API_BASE}/tv/{tmdb_id}" if tmdb_id else _search_tv_url(query))
    return _to_result(data, external_provider="tmdb", external_key="id", external_id=data.get("id"))


def _fetch_episode(query: jellyplug.MetadataQuery) -> MetadataResult:
    """Episodes are resolved via their show, then matched on season/episode.

    The host passes the episode title, but an episode title alone is a weak
    signal, so prefer an NFO-provided tmdb id and fall back to the show title
    with season/episode numbers from the item path.
    """
    episode_id = query.known_id("tmdb")
    if episode_id:
        return _to_result(_get(f"{API_BASE}/tv/episode/{episode_id}"), external_id=episode_id)

    # Without ids, use the show title from the enclosing directory.
    show_title = query.title
    season = episode_number_from(query.title, key="season")
    episode = episode_number_from(query.title, key="episode")

    if season is None or episode is None:
        jellyplug.log(
            "debug",
            f"cannot resolve episode {query.title!r}: no season/episode numbers available",
        )
        return MetadataResult()

    show = _get(_search_tv_url(jellyplug.MetadataQuery(title=show_title, year=query.year)))
    show_id = show.get("id")
    if not show_id:
        return MetadataResult()

    episode_data = _get(f"{API_BASE}/tv/{show_id}/episode/{season}/{episode}")
    result = _to_result(episode_data)
    # Preserve the show's identity so episodes can be linked to a series.
    result.external_ids["tmdb"] = str(show_id)
    return result


def episode_number_from(text: str, *, key: str) -> int | None:
    """Pull a season or episode number out of a name like ``S01E02``."""
    import re

    pattern = r"s(\d+)" if key == "season" else r"e(\d+)"
    match = re.search(pattern, text.lower())
    if match:
        return int(match.group(1))
    # Fall back to the x notation: 01x02
    match = re.search(r"(\d+)x(\d+)", text.lower())
    if match:
        return int(match.group(1) if key == "season" else 2)
    return None


def _search_movie_url(query: jellyplug.MetadataQuery) -> str:
    params = {"query": query.title, "include_adult": "false"}
    if query.year:
        params["year"] = str(query.year)
    from urllib.parse import urlencode

    return f"{API_BASE}/search/movie?{urlencode(params)}"


def _search_tv_url(query: jellyplug.MetadataQuery) -> str:
    params = {"query": query.title, "include_adult": "false"}
    if query.year:
        params["first_air_date_year"] = str(query.year)
    from urllib.parse import urlencode

    return f"{API_BASE}/search/tv?{urlencode(params)}"


def _get(url: str) -> dict:
    """GET a TMDB endpoint, unwrapping the ``results`` envelope for searches."""
    data = fetch_json(url, headers=headers(), timeout=REQUEST_TIMEOUT)

    if "results" in data:
        results = data["results"]
        if not results:
            raise HttpError(404, url, "no results")
        best = _best_match(results, url)
        # Search results carry only summaries; re-fetch the full record so
        # overview, genres, and runtime are present.
        kind = "movie" if "/search/movie" in url else "tv"
        ident = best.get("id")
        return fetch_json(
            f"{API_BASE}/{kind}/{ident}",
            headers=headers(),
            timeout=REQUEST_TIMEOUT,
        ) if ident else best

    return data


def _best_match(results: list[dict], url: str) -> dict:
    """Pick the most likely result.

    TMDB orders by popularity, which is a poor proxy for "the film the user
    meant". Prefer an exact title match, then the highest vote average among
    reasonably popular entries.
    """
    needle = url.lower()
    # Recover the query term from the URL for comparison.
    from urllib.parse import parse_qs, urlparse

    term = (parse_qs(urlparse(url).query).get("query") or [""])[0].lower()

    if term:
        exact = [r for r in results if (r.get("title") or r.get("name") or "").lower() == term]
        if exact:
            return max(exact, key=lambda r: r.get("vote_average") or 0)

    popular = [r for r in results if (r.get("popularity") or 0) > 1.0]
    pool = popular or results
    return max(pool, key=lambda r: (r.get("vote_count") or 0) * (r.get("vote_average") or 0))


# --------------------------------------------------------------------------
# Mapping
# --------------------------------------------------------------------------


def _to_result(
    data: dict,
    *,
    external_provider: str | None = None,
    external_key: str = "id",
    external_id: object = None,
) -> MetadataResult:
    """Map a TMDB record onto the host's metadata shape.

    TMDB uses one schema for movies and another for TV; both are normalized
    here so callers do not branch on record type.
    """
    is_tv = "first_air_date" in data or "episode_run_time" in data

    title = data.get("title") or data.get("name")
    release = (
        data.get("release_date")
        or data.get("first_air_date")
        or data.get("air_date")
        or ""
    )
    year = _year_of(release)
    runtime = data.get("runtime") or data.get("episode_run_time")

    people: list[MetadataPerson] = []
    for credit_key, kind in (
        ("credits", PersonKind.ACTOR),
        ("crew", PersonKind.DIRECTOR),
    ):
        for entry in (data.get(credit_key) or {}).get("cast" if kind == PersonKind.ACTOR else "directors") or []:
            name = entry.get("name")
            if name:
                people.append(
                    MetadataPerson(
                        name=name,
                        role=(entry.get("character") or entry.get("job") or "").strip(),
                        kind=kind,
                    )
                )

    images: list[MetadataImage] = []
    poster = data.get("poster_path")
    if poster:
        images.append(
            MetadataImage(
                path=f"{IMAGE_BASE}/w780{poster}",
                kind=ImageKind.PRIMARY,
                width=int(data.get("poster_width") or 0) or None,
                height=int(data.get("poster_height") or 0) or None,
            )
        )
    for index, backdrop in enumerate(data.get("backdrop_path") or []):
        images.append(
            MetadataImage(
                path=f"{IMAGE_BASE}/w1280{backdrop}",
                kind=ImageKind.BACKDROP,
                index=index,
            )
        )
    still = data.get("still_path")
    if still:
        # An episode's "poster" is really its still frame.
        images.append(
            MetadataImage(path=f"{IMAGE_BASE}/w780{still}", kind=ImageKind.THUMB)
        )

    external_ids: dict[str, str] = {}
    if external_id is not None:
        external_ids[external_provider or "tmdb"] = str(external_id)
    imdb = data.get("imdb_id") or (data.get("external_ids") or {}).get("imdb_id")
    if imdb:
        external_ids["imdb"] = imdb
    tvdb = (data.get("external_ids") or {}).get("tvdb_id")
    if tvdb:
        external_ids["tvdb"] = str(tvdb)

    genres = [
        GENRE_NAMES[g["id"]]
        for g in data.get("genres") or []
        if isinstance(g, dict) and g.get("id") in GENRE_NAMES
    ]

    result = MetadataResult(
        title=title,
        original_title=data.get("original_title") or data.get("original_name"),
        overview=data.get("overview") or None,
        tagline=data.get("tagline") or None,
        year=year,
        premiere_date=release or None,
        community_rating=data.get("vote_average") or None,
        official_rating=_certification(data),
        status="Released" if release else None,
        genres=genres,
        studios=[c["name"] for c in data.get("production_companies") or [] if c.get("name")][:3],
        run_time_ticks=(runtime * 60 * 10_000_000) if runtime else None,
        people=people[:40],
        images=images,
        external_ids=external_ids,
    )

    if is_tv:
        result.status = _tv_status(data.get("status"))
        # Series carry per-season runtimes, not a single value.
        if result.run_time_ticks is None and data.get("episode_run_time"):
            result.run_time_ticks = data["episode_run_time"] * 60 * 10_000_000

    return result


def _year_of(date_str: str) -> int | None:
    if len(date_str) >= 4 and date_str[:4].isdigit():
        return int(date_str[:4])
    return None


def _certification(data: dict) -> str | None:
    """Extract a US certification if TMDB has one."""
    for entry in data.get("content_ratings", {}).get("results", []) or []:
        if entry.get("iso_3166_1") == "US":
            return entry.get("rating")
    for entry in data.get("release_dates", {}).get("results", []) or []:
        for item in entry.get("release_dates") or []:
            if item.get("iso_3166_1") == "US" and item.get("certification"):
                return item["certification"]
    return None


def _tv_status(status: str | None) -> str | None:
    return {
        "Returning Series": "Continuing",
        "Ended": "Ended",
    }.get(status or "", status)


def on_event(event_json: str) -> None:
    """React to host events. Must return fast and never raise."""
    try:
        event = jellyplug.parse_event(event_json)
    except Exception:  # noqa: BLE001
        return

    if event.type == "item_discovered":
        # Provider plugins are pulled on demand by the host rather than eagerly
        # here, so this only records interest for future prefetching.
        jellyplug.log("debug", f"item discovered: {event.payload.get('item_id')}")

    elif event.type == "user_authenticated":
        # A good place to refresh a short-lived TMDB token, if the plugin ever
        # grows one.
        pass


def teardown() -> None:
    """Called at shutdown. Release anything held here."""
    pass