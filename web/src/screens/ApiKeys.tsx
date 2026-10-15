import { useEffect, useState, type FormEvent } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { navigate } from "../router";
import { getOmdbKey, omdbRequest, removeOmdbKey, saveOmdbKey, type OmdbTitle } from "../omdb";

export function ApiKeys() {
  const { session } = useSession();
  const userId = session?.user.Id ?? "";
  const isAdmin = session?.user.Policy.IsAdministrator ?? false;
  const [keyInput, setKeyInput] = useState("");
  const [hasKey, setHasKey] = useState(() => Boolean(userId && getOmdbKey(userId)));
  const [serverHasKey, setServerHasKey] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [term, setTerm] = useState("");
  const [results, setResults] = useState<OmdbTitle[]>([]);

  useEffect(() => {
    // The OMDb key lives in server-wide plugin configuration and the endpoint
    // is administrator-only; do not ask for it on a member's behalf.
    if (!userId || !isAdmin) return;
    void api.omdbApiKeyStatus()
      .then((status) => setServerHasKey(status.Configured))
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : String(err));
      });
  }, [userId, isAdmin]);

  async function saveKey(event: FormEvent) {
    event.preventDefault();
    setError("");
    setMessage("");
    setBusy(true);
    try {
      const status = await api.saveOmdbApiKey(keyInput);
      saveOmdbKey(userId, keyInput);
      setHasKey(true);
      setServerHasKey(status.Configured);
      setKeyInput("");
      setMessage("OMDb key saved on this server and enabled for library metadata.");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function clearKey() {
    setError("");
    setMessage("");
    setBusy(true);
    try {
      const status = await api.removeOmdbApiKey();
      removeOmdbKey(userId);
      setServerHasKey(status.Configured);
      setHasKey(false);
      setKeyInput("");
      setResults([]);
      setMessage(status.Configured
        ? "Saved OMDb key removed. An OMDB_API_KEY environment setting is still active."
        : "OMDb key removed from the server.");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function testKey() {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await omdbRequest(getOmdbKey(userId), { i: "tt3896198" });
      setMessage("OMDb connection successful. Your API key is working.");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function searchTitles(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    setMessage("");
    setResults([]);
    try {
      const result = await omdbRequest(getOmdbKey(userId), { s: term.trim() });
      setResults(result.Search ?? []);
      if (!result.Search?.length) setMessage("No matching titles found.");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  // A hidden sidebar entry is not a boundary: the route is shareable, so a
  // member who reaches it directly gets the same refusal the server gives.
  if (!isAdmin) {
    return (
      <main>
        <p className="error">API key management requires administrator access.</p>
      </main>
    );
  }

  return (
    <main>
      <div className="section-head">
        <button type="button" className="link" onClick={() => navigate({ name: "home" })}>
          ← Home
        </button>
        <h1>API Keys</h1>
      </div>

      <section className="api-settings-card">
        <span className="catalog-kicker">Personal integration</span>
        <h2>OMDb API</h2>
        <p className="muted">
          Save your OMDb API key on the server to enable metadata lookup for your
          movie and TV library.
        </p>

        <form className="api-key-form" onSubmit={saveKey}>
          <label className="field">
            <span>{serverHasKey ? "Replace API key" : "OMDb API key"}</span>
            <input
              type="password"
              autoComplete="new-password"
              placeholder={serverHasKey ? "A key is saved; enter a replacement" : "Paste your OMDb API key"}
              value={keyInput}
              onChange={(event) => setKeyInput(event.target.value)}
            />
          </label>
          <div className="api-key-actions">
            <button type="submit" className="primary" disabled={!keyInput.trim() || busy}>
              Save key
            </button>
            {hasKey && (
              <>
                <button type="button" className="ghost" onClick={() => void testKey()} disabled={busy}>
                  {busy ? "Testing…" : "Test key"}
                </button>
              </>
            )}
            {serverHasKey && (
              <button type="button" className="link danger" onClick={() => void clearKey()} disabled={busy}>
                Remove key
              </button>
            )}
          </div>
        </form>

        {error && <p className="error" role="alert">{error}</p>}
        {message && <p className="api-key-message" role="status">{message}</p>}
        <p className="muted small api-key-note">
          {serverHasKey
            ? "An OMDb API key is configured on the server. It is stored in the server database and never returned to this page."
            : "No OMDb key is configured on the server. The key is stored in the server database and used by the metadata provider."}
          {" "}You can also set <code>OMDB_API_KEY</code> in the server environment.
        </p>
      </section>

      <section className="api-settings-card">
        <span className="catalog-kicker">Metadata lookup</span>
        <h2>Search OMDb</h2>
        <form className="omdb-search-form" onSubmit={(event) => void searchTitles(event)}>
          <label className="field grow">
            <span>Title</span>
            <input
              type="search"
              placeholder="Search for a movie or series"
              value={term}
              onChange={(event) => setTerm(event.target.value)}
              disabled={!hasKey || busy}
              required
            />
          </label>
          <button type="submit" className="primary" disabled={!hasKey || busy || !term.trim()}>
            {busy ? "Searching…" : "Search"}
          </button>
        </form>
        {!hasKey && <p className="muted small">Save an OMDb key above to enable search.</p>}
        {results.length > 0 && (
          <ul className="omdb-results">
            {results.map((result) => (
              <li key={result.imdbID}>
                <strong>{result.Title}</strong>
                <span className="muted">{result.Year} · {result.Type}</span>
                <code>{result.imdbID}</code>
              </li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}
