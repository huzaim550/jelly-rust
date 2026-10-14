const OMDB_ENDPOINT = "https://www.omdbapi.com/";

export interface OmdbTitle {
  Title: string;
  Year: string;
  imdbID: string;
  Type: string;
  Poster: string;
}

interface OmdbResponse {
  Response: "True" | "False";
  Error?: string;
  Search?: OmdbTitle[];
}

function storageKey(userId: string): string {
  return `jf.omdb.apiKey.${encodeURIComponent(userId)}`;
}

export function getOmdbKey(userId: string): string {
  return window.localStorage.getItem(storageKey(userId)) ?? "";
}

export function saveOmdbKey(userId: string, key: string): void {
  const trimmed = key.trim();
  if (!trimmed) throw new Error("Enter an OMDb API key.");
  window.localStorage.setItem(storageKey(userId), trimmed);
}

export function removeOmdbKey(userId: string): void {
  window.localStorage.removeItem(storageKey(userId));
}

export async function omdbRequest(
  key: string,
  params: Record<string, string>,
): Promise<OmdbResponse> {
  if (!key.trim()) throw new Error("Save an OMDb API key before making requests.");
  const query = new URLSearchParams({ ...params, apikey: key });
  const response = await fetch(`${OMDB_ENDPOINT}?${query.toString()}`, {
    headers: { Accept: "application/json" },
  });
  if (!response.ok) {
    throw new Error(`OMDb request failed with HTTP ${response.status}.`);
  }

  const result: unknown = await response.json();
  if (!isOmdbResponse(result)) throw new Error("OMDb returned an unexpected response.");
  if (result.Response === "False") {
    throw new Error(result.Error || "OMDb rejected the request.");
  }
  return result;
}

function isOmdbResponse(value: unknown): value is OmdbResponse {
  if (typeof value !== "object" || value === null || !("Response" in value)) return false;
  if (value.Response !== "True" && value.Response !== "False") return false;
  if ("Error" in value && typeof value.Error !== "string") return false;
  if (
    "Search" in value &&
    (!Array.isArray(value.Search) ||
      !value.Search.every(
        (title) =>
          typeof title === "object" &&
          title !== null &&
          "Title" in title &&
          typeof title.Title === "string" &&
          "Year" in title &&
          typeof title.Year === "string" &&
          "imdbID" in title &&
          typeof title.imdbID === "string" &&
          "Type" in title &&
          typeof title.Type === "string" &&
          "Poster" in title &&
          typeof title.Poster === "string",
      ))
  ) {
    return false;
  }
  return true;
}
