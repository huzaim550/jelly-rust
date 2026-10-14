/**
 * TMDB metadata provider.
 *
 * The reference implementation of a metadata plugin. Set an API key from the
 * dashboard, or via `TMDB_API_KEY` for unattended setups.
 *
 * TMDB uses two credential styles: a short v3 API key, which goes in the
 * `api_key` query parameter, and a long v4 read token, which is a JWT and must
 * be sent as a Bearer header. Both are accepted.
 */

import {
  definePlugin,
  fetchJson,
  host,
  requireCapability,
  type MetadataQuery,
  type MetadataResult,
  type PluginEvent,
} from "@jellyfin-clone/pluginhost/sdk";

const API_BASE = "https://api.themoviedb.org/3";
const IMAGE_BASE = "https://image.tmdb.org/t/p";

/** Per-request deadline, kept well under the host's own budget. */
const TIMEOUT_MS = 8_000;

/** TMDB genre ids that map onto Jellyfin genres. Unknown ids are dropped. */
const GENRE_NAMES: Record<number, string> = {
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
};

interface TmdbMovie {
  id: number;
  title?: string;
  original_title?: string;
  name?: string;
  original_name?: string;
  overview?: string | null;
  tagline?: string | null;
  release_date?: string;
  first_air_date?: string;
  air_date?: string;
  poster_path?: string | null;
  backdrop_path?: string | null;
  still_path?: string | null;
  poster_width?: number;
  poster_height?: number;
  runtime?: number | null;
  episode_run_time?: number[];
  vote_average?: number;
  vote_count?: number;
  popularity?: number;
  status?: string;
  genres?: Array<{ id: number; name: string }>;
  production_companies?: Array<{ id: number; name: string }>;
  credits?: {
    cast?: Array<{ name: string; character?: string }>;
    crew?: Array<{ name: string; job?: string }>;
  };
  imdb_id?: string | null;
  external_ids?: { imdb_id?: string; tvdb_id?: number };
  content_ratings?: { results: Array<{ iso_3166_1: string; rating: string }> };
  release_dates?: {
    results: Array<{ iso_3166_1: string; release_date?: string }>;
  };
}

interface TmdbSearchResult {
  results: TmdbMovie[];
}

async function apiKey(): Promise<string | undefined> {
  const stored = await config_get("api_key");
  if (stored) return stored;
  return process.env.TMDB_API_KEY || process.env.TMDB_API_TOKEN || undefined;
}

/** Auth headers, or query parameters for the v3 key style. */
function authHeaders(key: string): Record<string, string> {
  // A v4 read token is a JWT; a v3 key is a short hex string.
  if (key.includes(".")) return { Authorization: `Bearer ${key}` };
  return {};
}

function authQuery(key: string): Record<string, string> {
  return key.includes(".") ? {} : { api_key: key };
}

async function config_get(key: string): Promise<string | undefined> {
  try {
    return await configGetSafe(key);
  } catch {
    return undefined;
  }
}

/**
 * Read plugin config, tolerating an unavailable bridge.
 *
 * Config lives in the server database, so it is unavailable when this module is
 * imported outside a running host (tests, `node --inspect`). That must not stop
 * the plugin loading.
 */
async function configGetSafe(key: string): Promise<string | undefined> {
  const value = await host.configGet(key);
  return value ?? undefined;
}

async function tmdbGet<T>(path: string, params: Record<string, string> = {}): Promise<T> {
  const key = await apiKey();
  if (!key) throw new Error("no TMDB API key configured");

  const search = new URLSearchParams({ ...params, ...authQuery(key) });
  const separator = path.includes("?") ? "&" : "?";
  return fetchJson<T>(`${API_BASE}${path}${separator}${search.toString()}`, {
    headers: authHeaders(key),
    timeoutMs: TIMEOUT_MS,
  });
}

/** Expand a search result into the full record, which carries the useful data. */
async function expand(summary: TmdbMovie, kind: "movie" | "tv"): Promise<TmdbMovie> {
  if (!summary.id) return summary;
  return tmdbGet<TmdbMovie>(`/${kind}/${summary.id}`);
}

/**
 * Pick the likeliest search hit.
 *
 * TMDB orders by popularity, which is a poor proxy for "the one the user meant".
 * Prefer an exact title match, then the highest-rated result that anyone has
 * actually seen.
 */
function bestMatch(results: TmdbMovie[], term: string): TmdbMovie {
  const needle = term.trim().toLowerCase();

  const exact = results.filter((r) => (r.title ?? r.name ?? "").toLowerCase() === needle);
  const pool = exact.length > 0 ? exact : results.filter((r) => (r.popularity ?? 0) > 1);

  const candidates = pool.length > 0 ? pool : results;
  return candidates.reduce((best, current) => {
    const score = (r: TmdbMovie) => (r.vote_count ?? 0) * (r.vote_average ?? 0);
    return score(current) > score(best) ? current : best;
  });
}

async function searchMovie(query: MetadataQuery): Promise<TmdbMovie> {
  const params: Record<string, string> = { query: query.title, include_adult: "false" };
  if (query.year) params.year = String(query.year);

  const data = await tmdbGet<TmdbSearchResult>("/search/movie", params);
  if (!data.results?.length) throw new Error(`no TMDB match for ${query.title}`);

  return expand(bestMatch(data.results, query.title), "movie");
}

async function searchSeries(query: MetadataQuery): Promise<TmdbMovie> {
  const params: Record<string, string> = { query: query.title, include_adult: "false" };
  if (query.year) params.first_air_date_year = String(query.year);

  const data = await tmdbGet<TmdbSearchResult>("/search/tv", params);
  if (!data.results?.length) throw new Error(`no TMDB match for ${query.title}`);

  return expand(bestMatch(data.results, query.title), "tv");
}

async function fetchMovie(query: MetadataQuery): Promise<MetadataResult> {
  const known = query.knownIds?.tmdb;
  const movie = known
    ? await tmdbGet<TmdbMovie>(`/movie/${known}`)
    : await searchMovie(query);

  const result = toResult(movie);
  if (movie.id) result.externalIds = { ...result.externalIds, tmdb: String(movie.id) };
  return result;
}

async function fetchSeries(query: MetadataQuery): Promise<MetadataResult> {
  const known = query.knownIds?.tmdb;
  const series = known
    ? await tmdbGet<TmdbMovie>(`/tv/${known}`)
    : await searchSeries(query);

  const result = toResult(series);
  if (series.id) result.externalIds = { ...result.externalIds, tmdb: String(series.id) };
  return result;
}

/**
 * Resolve an episode through its series, then match on season and number.
 *
 * An episode title alone is a weak signal — "The One" matches hundreds of
 * things — so this prefers a known tmdb id, then a series name plus numbers.
 */
async function fetchEpisode(query: MetadataQuery): Promise<MetadataResult> {
  const known = query.knownIds?.tmdb;
  if (known) {
    const episode = await tmdbGet<TmdbMovie>(`/tv/episode/${known}`);
    const result = toResult(episode);
    result.externalIds = { ...result.externalIds, tmdb: known };
    return result;
  }

  const season = extractNumber(query.title, "season");
  const episodeNumber = extractNumber(query.title, "episode");
  if (season === undefined || episodeNumber === undefined) {
    host.log("debug", `cannot resolve episode "${query.title}": no season/episode numbers`);
    return {};
  }

  const show = await searchSeries({ ...query, title: query.title, kind: "Series" });
  if (!show.id) return {};

  const episode = await tmdbGet<TmdbMovie>(
    `/tv/${show.id}/episode/${season}/${episodeNumber}`,
  );
  const result = toResult(episode);
  // Keep the show's id so episodes can be linked back to a series.
  result.externalIds = { ...result.externalIds, tmdb: String(show.id) };
  return result;
}

/** Pull a season or episode number out of a name like `S01E02`. */
function extractNumber(text: string, which: "season" | "episode"): number | undefined {
  const letter = which === "season" ? "s" : "e";
  const direct = new RegExp(`${letter}(\\d+)`, "i").exec(text);
  if (direct?.[1]) return Number(direct[1]);

  // Fall back to the `01x02` notation.
  const crossed = /(\d+)x(\d+)/i.exec(text);
  if (crossed) return Number(which === "season" ? crossed[1] : crossed[2]);
  return undefined;
}

/** Map a TMDB record onto the host's metadata shape. */
function toResult(data: TmdbMovie): MetadataResult {
  const isTv = Boolean(data.first_air_date || data.episode_run_time);

  const release = data.release_date || data.first_air_date || data.air_date || "";
  const runtime = data.runtime ?? data.episode_run_time?.[0];

  const people: NonNullable<MetadataResult["people"]> = [];
  for (const cast of data.credits?.cast ?? []) {
    if (cast.name) {
      people.push({ name: cast.name, role: cast.character ?? "", kind: "actor" });
    }
  }
  for (const member of data.credits?.crew ?? []) {
    if (member.name && member.job === "Director") {
      people.push({ name: member.name, role: "Director", kind: "director" });
    }
  }

  const images: NonNullable<MetadataResult["images"]> = [];
  if (data.poster_path) {
    images.push({
      path: `${IMAGE_BASE}/w780${data.poster_path}`,
      kind: "primary",
      width: data.poster_width,
      height: data.poster_height,
    });
  }
  // A backdrop is singular on TMDB; repeat it so the client has options.
  if (data.backdrop_path) {
    images.push({ path: `${IMAGE_BASE}/w1280${data.backdrop_path}`, kind: "backdrop" });
  }
  if (data.still_path) {
    // An episode's "poster" is really its still frame.
    images.push({ path: `${IMAGE_BASE}/w780${data.still_path}`, kind: "thumb" });
  }

  const externalIds: Record<string, string> = {};
  const imdb = data.imdb_id || data.external_ids?.imdb_id;
  if (imdb) externalIds.imdb = imdb;
  if (data.external_ids?.tvdb_id) externalIds.tvdb = String(data.external_ids.tvdb_id);

  const genres = (data.genres ?? [])
    .filter((g) => GENRE_NAMES[g.id] !== undefined)
    .map((g) => GENRE_NAMES[g.id] as string);

  return {
    title: data.title || data.name,
    originalTitle: data.original_title || data.original_name,
    overview: data.overview || undefined,
    tagline: data.tagline || undefined,
    year: release ? Number(release.slice(0, 4)) : undefined,
    premiereDate: release || undefined,
    communityRating: data.vote_average || undefined,
    officialRating: certification(data),
    status: isTv ? tvStatus(data.status) : release ? "Released" : undefined,
    genres,
    studios: (data.production_companies ?? []).slice(0, 3).map((c) => c.name),
    // TMDB reports minutes; the host counts 100ns ticks.
    runTimeTicks: runtime ? runtime * 60 * 10_000_000 : undefined,
    people: people.slice(0, 40),
    images,
    externalIds,
  };
}

function certification(data: TmdbMovie): string | undefined {
  for (const entry of data.content_ratings?.results ?? []) {
    if (entry.iso_3166_1 === "US") return entry.rating;
  }
  for (const entry of data.release_dates?.results ?? []) {
    if (entry.iso_3166_1 === "US" && entry.release_date) return entry.release_date;
  }
  return undefined;
}

function tvStatus(status: string | undefined): string | undefined {
  if (!status) return undefined;
  return (
    {
      "Returning Series": "Continuing",
      Ended: "Ended",
    }[status] ?? status
  );
}

export default definePlugin({
  setup() {
    requireCapability("network");
    void apiKey().then((key) => {
      if (!key) {
        host.log(
          "warn",
          "TMDB plugin loaded without an API key; set one in the dashboard or " +
            "TMDB_API_KEY to enable metadata fetching.",
        );
      }
    });
  },

  async fetchMetadata(query) {
    if (!query.title?.trim()) return undefined;

    if (!(await apiKey())) {
      host.log("debug", `skipping ${query.title}: no TMDB API key`);
      return undefined;
    }

    try {
      if (query.kind === "Episode") return await fetchEpisode(query);
      if (query.kind === "Series") return await fetchSeries(query);
      return await fetchMovie(query);
    } catch (error) {
      // A provider that throws must not break the chain for other providers,
      // so this is reported and turned into a decline.
      const message = error instanceof Error ? error.message : String(error);
      host.log("warn", `TMDB lookup failed for ${query.title}: ${message}`);
      return undefined;
    }
  },

  onEvent(event: PluginEvent) {
    // Metadata is fetched on demand by the host rather than eagerly here, so
    // this only records interest for a future prefetcher.
    if (event.type === "itemDiscovered") {
      host.log("debug", `item discovered: ${event.itemId}`);
    }
  },
});