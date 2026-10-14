/**
 * OMDb metadata provider. Configure an API key as `OMDB_API_KEY` or in the
 * plugin's `api_key` setting.
 */

import {
  definePlugin,
  fetchJson,
  host,
  requireCapability,
  type MetadataQuery,
  type MetadataResult,
} from "@jellyfin-clone/pluginhost/sdk";

const API_URL = "https://www.omdbapi.com/";
const TIMEOUT_MS = 8_000;

interface OmdbRecord {
  Response: "True" | "False";
  Error?: string;
  Search?: Array<{
    Title: string;
    Year: string;
    imdbID: string;
    Type: string;
    Poster: string;
  }>;
  Title?: string;
  Year?: string;
  imdbID?: string;
  Type?: string;
  Rated?: string;
  Released?: string;
  Runtime?: string;
  Genre?: string;
  Director?: string;
  Writer?: string;
  Actors?: string;
  Plot?: string;
  Poster?: string;
  imdbRating?: string;
}

async function apiKey(): Promise<string | undefined> {
  try {
    const stored = await host.configGet("api_key");
    if (stored) return stored;
  } catch {
    // The host bridge is unavailable when the plugin is inspected outside the server.
  }
  return process.env.OMDB_API_KEY || undefined;
}

async function omdbGet(params: Record<string, string>): Promise<OmdbRecord> {
  const key = await apiKey();
  if (!key) throw new Error("no OMDb API key configured");

  const search = new URLSearchParams({ ...params, apikey: key });
  const result = await fetchJson<OmdbRecord>(`${API_URL}?${search.toString()}`, {
    headers: { Accept: "application/json" },
    timeoutMs: TIMEOUT_MS,
  });
  if (result.Response !== "True") {
    throw new Error(result.Error || "OMDb returned an unsuccessful response");
  }
  return result;
}

async function search(query: MetadataQuery, type: "movie" | "series"): Promise<OmdbRecord> {
  const titles = [...new Set([normalizeSearchTitle(query.title), query.title.trim()])];
  let matches: NonNullable<OmdbRecord["Search"]> = [];
  let searchedTitle = query.title;

  for (const title of titles) {
    const params: Record<string, string> = { s: title, type };
    if (query.year) params.y = String(query.year);

    try {
      const result = await omdbGet(params);
      matches = result.Search ?? [];
      searchedTitle = title;
      if (matches.length > 0) break;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (!/not found/i.test(message)) throw error;
    }
  }

  if (matches.length === 0) throw new Error(`no OMDb match for ${query.title}`);

  const normalizedTitle = normalizeSearchTitle(searchedTitle).toLowerCase();
  const exact = matches.filter(
    (match) => normalizeSearchTitle(match.Title).toLowerCase() === normalizedTitle,
  );
  const candidates = exact.length > 0 ? exact : matches;
  const yearMatch = query.year
    ? candidates.find((match) => match.Year.startsWith(String(query.year)))
    : undefined;
  const match = yearMatch ?? candidates[0];
  if (!match) throw new Error(`no OMDb match for ${query.title}`);
  return omdbGet({ i: match.imdbID, plot: "full" });
}

function normalizeSearchTitle(title: string): string {
  return title
    .replace(/\s*\[(?:hindi|tamil|telugu|malayalam|kannada|bengali|punjabi|marathi|gujarati|urdu|english)\]\s*$/i, "")
    .trim();
}

async function fetchItem(query: MetadataQuery, type: "movie" | "series"): Promise<OmdbRecord> {
  const knownImdbId = query.knownIds?.imdb;
  return knownImdbId
    ? omdbGet({ i: knownImdbId, plot: "full" })
    : search(query, type);
}

async function fetchEpisode(query: MetadataQuery): Promise<MetadataResult> {
  const knownImdbId = query.knownIds?.imdb;
  if (knownImdbId) return toResult(await omdbGet({ i: knownImdbId, plot: "full" }));

  const parsed = parseEpisode(query.title);
  if (!parsed) {
    host.log("debug", `cannot resolve episode "${query.title}": no season/episode numbers`);
    return {};
  }

  const episode = await omdbGet({
    t: parsed.seriesTitle,
    Season: String(parsed.season),
    Episode: String(parsed.episode),
    plot: "full",
  });
  return toResult(episode);
}

function parseEpisode(title: string): { seriesTitle: string; season: number; episode: number } | undefined {
  const match = /^(.*?)\s*(?:s(\d{1,2})e(\d{1,3})|(\d{1,2})x(\d{1,3}))\b/i.exec(title.trim());
  if (!match) return undefined;

  const seriesTitle = match[1]?.trim();
  const season = Number(match[2] ?? match[4]);
  const episode = Number(match[3] ?? match[5]);
  if (!seriesTitle || !Number.isInteger(season) || !Number.isInteger(episode)) return undefined;
  return { seriesTitle, season, episode };
}

function toResult(data: OmdbRecord): MetadataResult {
  const runtime = /^(\d+)\s*min$/i.exec(data.Runtime ?? "")?.[1];
  const rating = Number(data.imdbRating);
  const people: NonNullable<MetadataResult["people"]> = [];

  for (const name of splitNames(data.Actors)) people.push({ name, role: "Actor", kind: "actor" });
  for (const name of splitNames(data.Director)) people.push({ name, role: "Director", kind: "director" });
  for (const name of splitNames(data.Writer)) people.push({ name, role: "Writer", kind: "writer" });

  const images: NonNullable<MetadataResult["images"]> = [];
  if (data.Poster && data.Poster !== "N/A") {
    images.push({ path: data.Poster, kind: "primary" });
  }

  const result: MetadataResult = {
    title: data.Title || undefined,
    originalTitle: data.Title || undefined,
    overview: data.Plot && data.Plot !== "N/A" ? data.Plot : undefined,
    year: parseYear(data.Year),
    premiereDate: toIsoDate(data.Released),
    communityRating: Number.isFinite(rating) && rating > 0 ? rating : undefined,
    officialRating: data.Rated && data.Rated !== "N/A" ? data.Rated : undefined,
    genres: splitNames(data.Genre),
    runTimeTicks: runtime ? Number(runtime) * 60 * 10_000_000 : undefined,
    people: people.slice(0, 40),
    images,
    externalIds: data.imdbID ? { imdb: data.imdbID } : {},
  };
  return result;
}

function splitNames(value: string | undefined): string[] {
  return value && value !== "N/A" ? value.split(",").map((name) => name.trim()).filter(Boolean) : [];
}

function parseYear(value: string | undefined): number | undefined {
  const year = /^(\d{4})/.exec(value ?? "")?.[1];
  return year ? Number(year) : undefined;
}

function toIsoDate(value: string | undefined): string | undefined {
  const match = /^(\d{1,2}) ([A-Za-z]{3}) (\d{4})$/.exec(value ?? "");
  if (!match) return undefined;

  const months: Record<string, string> = {
    jan: "01", feb: "02", mar: "03", apr: "04", may: "05", jun: "06",
    jul: "07", aug: "08", sep: "09", oct: "10", nov: "11", dec: "12",
  };
  const month = months[match[2]?.toLowerCase() ?? ""];
  return month ? `${match[3]}-${month}-${(match[1] ?? "").padStart(2, "0")}` : undefined;
}

export default definePlugin({
  setup() {
    requireCapability("network");
    void apiKey().then((key) => {
      if (!key) host.log("warn", "OMDb plugin loaded without an API key; set OMDB_API_KEY or configure the plugin api_key.");
    });
  },

  async fetchMetadata(query) {
    if (!query.title?.trim()) return undefined;

    if (!(await apiKey())) {
      host.log("debug", `skipping ${query.title}: no OMDb API key`);
      return undefined;
    }

    try {
      if (query.kind === "Episode") return await fetchEpisode(query);
      const record = await fetchItem(query, query.kind === "Series" ? "series" : "movie");
      return toResult(record);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (/not found|no OMDb match/i.test(message)) {
        host.log("debug", `no OMDb match for ${query.title}`);
        return {};
      }
      host.log("warn", `OMDb lookup failed for ${query.title}: ${message}`);
      throw error;
    }
  },
});
