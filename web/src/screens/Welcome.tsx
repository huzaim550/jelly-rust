import { useCallback, useEffect, useState } from "react";
import { api } from "../api/client";
import { LIBRARY_TYPES, type LibraryType, type VirtualFolder } from "../library";
import type { Route } from "../router";

/**
 * First-run onboarding.
 *
 * Four steps, mirroring Jellyfin's own wizard: welcome, administrator account,
 * libraries, done. The flow is driven by server state rather than a local flag,
 * so a half-finished setup resumes where it left off after a reload.
 */
export function Welcome({ go }: { go: (route: Route, replace?: boolean) => void }) {
  const [step, setStep] = useState(0);

  return (
    <div className="wizard">
      <WizardHeader current={step} />
      {step === 0 && <StepWelcome onNext={() => setStep(1)} />}
      {step === 1 && <StepServer onNext={() => setStep(2)} />}
      {step === 2 && <StepAdmin onNext={() => setStep(3)} />}
      {step === 3 && <StepLibraries onDone={() => go({ name: "home" }, true)} />}
    </div>
  );
}

const STEPS = [
  { key: "welcome", label: "Welcome" },
  { key: "server", label: "Server" },
  { key: "admin", label: "Administrator" },
  { key: "libraries", label: "Libraries" },
];

function WizardHeader({ current }: { current: number }) {
  return (
    <div className="wizard-header">
      <div className="wizard-brand">
        <span className="logo">◐</span>
        <span>jellyfin-clone</span>
      </div>
      <ol className="wizard-steps">
        {STEPS.map((s, i) => (
          <li key={s.key} className={i === current ? "active" : i < current ? "done" : ""}>
            <span className="dot">{i < current ? "✓" : i + 1}</span>
            {s.label}
          </li>
        ))}
      </ol>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 1 — welcome
// ---------------------------------------------------------------------------

function StepWelcome({ onNext }: { onNext: () => void }) {
  return (
    <div className="wizard-body">
      <h1>Welcome to jellyfin-clone</h1>
      <p className="muted">Let&rsquo;s get your server set up. This takes about a minute.</p>

      <ul className="feature-list">
        <li>
          <strong>Your media, your server.</strong> Point it at folders on this machine and it
          builds a library from them.
        </li>
        <li>
          <strong>Play anywhere.</strong> The web client runs in your browser and adapts to the
          screen.
        </li>
        <li>
          <strong>Extend it.</strong> JavaScript plugins add metadata providers and more.
        </li>
      </ul>

      <div className="wizard-actions">
        <button type="button" className="primary large" onClick={onNext}>
          Get started
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 2 — server settings
// ---------------------------------------------------------------------------

const LANGUAGES = [
  { code: "en", label: "English", country: "US" },
  { code: "en", label: "English (United Kingdom)", country: "GB" },
  { code: "de", label: "Deutsch", country: "DE" },
  { code: "fr", label: "Français", country: "FR" },
  { code: "es", label: "Español", country: "ES" },
  { code: "ja", label: "日本語", country: "JP" },
  { code: "pt", label: "Português", country: "BR" },
];

function StepServer({ onNext }: { onNext: () => void }) {
  const [name, setName] = useState("jellyfin-clone");
  const [language, setLanguage] = useState(LANGUAGES[0]!);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api
      .startupConfiguration()
      .then((config) => {
        if (config.ServerName) setName(config.ServerName);
        const match = LANGUAGES.find(
          (l) => l.code === config.PreferredMetadataLanguage &&
            l.country === config.MetadataCountryCode,
        );
        if (match) setLanguage(match);
      })
      .catch(() => {
        // Defaults are fine if the server has nothing configured yet.
      });
  }, []);

  async function submit() {
    setBusy(true);
    setError("");
    try {
      await api.postStartupConfiguration({
        ServerName: name.trim() || "jellyfin-clone",
        PreferredMetadataLanguage: language.code,
        MetadataCountryCode: language.country,
        UICulture: `${language.code}-${language.country}`,
      });
      onNext();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="wizard-body">
      <h1>Server settings</h1>
      <p className="muted">How this server identifies itself, and which language metadata uses.</p>

      <label className="field">
        <span>Server name</span>
        <input
          value={name}
          maxLength={40}
          onChange={(e) => setName(e.target.value)}
          placeholder="jellyfin-clone"
        />
      </label>

      <label className="field">
        <span>Metadata language</span>
        <select
          value={`${language.code}-${language.country}`}
          onChange={(e) => {
            const found = LANGUAGES.find(
              (l) => `${l.code}-${l.country}` === e.target.value,
            );
            if (found) setLanguage(found);
          }}
        >
          {LANGUAGES.map((l) => (
            <option key={`${l.code}-${l.country}`} value={`${l.code}-${l.country}`}>
              {l.label}
            </option>
          ))}
        </select>
      </label>

      {error && <p className="error">{error}</p>}

      <div className="wizard-actions">
        <button type="button" className="primary large" onClick={() => void submit()} disabled={busy}>
          {busy ? "Saving…" : "Continue"}
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 3 — administrator
// ---------------------------------------------------------------------------

function StepAdmin({ onNext }: { onNext: () => void }) {
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const passwordsDiffer = confirm.length > 0 && password !== confirm;

  async function submit() {
    if (passwordsDiffer) {
      setError("The passwords do not match.");
      return;
    }

    setBusy(true);
    setError("");
    try {
      // An empty password creates a passwordless account: anyone who can reach
      // the server can sign in as it, which is a reasonable trade on a trusted
      // home network and is why the field is optional.
      await api.createStartupUser(name.trim(), password || undefined);
      onNext();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="wizard-body">
      <h1>Create an administrator</h1>
      <p className="muted">
        This account can manage libraries, users, and plugins. It is the only way to change those
        later.
      </p>

      <label className="field">
        <span>Username</span>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
          placeholder="admin"
        />
      </label>

      <label className="field">
        <span>
          Password <span className="muted">(optional)</span>
        </span>
        <div className="password-row">
          <input
            type={showPassword ? "text" : "password"}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          <button
            type="button"
            className="link"
            onClick={() => setShowPassword((v) => !v)}
          >
            {showPassword ? "Hide" : "Show"}
          </button>
        </div>
      </label>

      {password && (
        <label className="field">
          <span>Confirm password</span>
          <input
            type={showPassword ? "text" : "password"}
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </label>
      )}

      {passwordsDiffer && <p className="error">The passwords do not match.</p>}
      {!password && (
        <p className="hint">
          Leave the password empty to create a passwordless account. Anyone who can reach this
          server can then sign in as you.
        </p>
      )}
      {error && <p className="error">{error}</p>}

      <div className="wizard-actions">
        <button
          type="button"
          className="primary large"
          onClick={() => void submit()}
          disabled={busy || !name.trim() || passwordsDiffer}
        >
          {busy ? "Creating…" : "Create account"}
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 4 — libraries
// ---------------------------------------------------------------------------

function StepLibraries({ onDone }: { onDone: () => void }) {
  const [folders, setFolders] = useState<VirtualFolder[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api
      .virtualFolders()
      .then(setFolders)
      .catch(() => setFolders([]));
  }, []);

  async function finish() {
    setBusy(true);
    try {
      await api.completeStartup();
      onDone();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="wizard-body wide">
      <h1>Add your media libraries</h1>
      <p className="muted">
        Point the server at folders on this machine. You can add more later from the dashboard.
      </p>

      {folders.length > 0 && <FolderList folders={folders} onChange={setFolders} />}

      <LibraryPicker
        onAdded={async () => {
          setFolders(await api.virtualFolders().catch(() => []));
        }}
      />

      {error && <p className="error">{error}</p>}

      <div className="wizard-actions">
        <button type="button" className="primary large" onClick={() => void finish()} disabled={busy}>
          {busy ? "Finishing…" : "Finish setup"}
        </button>
        <button type="button" className="link" onClick={onDone}>
          Skip for now
        </button>
      </div>
    </div>
  );
}

/** Existing libraries, with remove. */
export function FolderList({
  folders,
  onChange,
}: {
  folders: VirtualFolder[];
  onChange: (folders: VirtualFolder[]) => void;
}) {
  const [removing, setRemoving] = useState<string | null>(null);

  const remove = useCallback(
    async (name: string) => {
      setRemoving(name);
      try {
        await api.deleteVirtualFolder(name);
        onChange(folders.filter((f) => f.Name !== name));
      } catch {
        // Leave the list as-is; the next refresh shows the truth.
      } finally {
        setRemoving(null);
      }
    },
    [folders, onChange],
  );

  if (folders.length === 0) return null;

  return (
    <ul className="folder-list">
      {folders.map((folder) => (
        <li key={folder.ItemId}>
          <span className="name">{folder.Name}</span>
          <span className="muted path" title={folder.Locations.join(", ")}>
            {folder.Locations[0]}
          </span>
          <span className="muted type">{folder.CollectionType ?? "mixed"}</span>
          <button
            type="button"
            className="link danger"
            onClick={() => void remove(folder.Name)}
            disabled={removing === folder.Name}
          >
            {removing === folder.Name ? "…" : "Remove"}
          </button>
        </li>
      ))}
    </ul>
  );
}

/** Add-library form with a server-side folder picker. */
export function LibraryPicker({
  onAdded,
}: {
  onAdded: () => void | Promise<void>;
}) {
  const [type, setType] = useState<LibraryType>("movies");
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [browsePath, setBrowsePath] = useState("/");
  const [entries, setEntries] = useState<Array<{ name: string; path: string }>>([]);
  const [browsing, setBrowsing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const browse = useCallback(async (target: string) => {
    setBrowsing(true);
    try {
      setEntries(await api.physicalPaths(target));
      setBrowsePath(target);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBrowsing(false);
    }
  }, []);

  useEffect(() => {
    void browse("/");
  }, [browse]);

  async function add() {
    if (!path.trim()) {
      setError("Choose a folder on the server.");
      return;
    }

    setBusy(true);
    setError("");
    try {
      const label =
        name.trim() || path.split("/").filter(Boolean).pop() || "Library";
      await api.addVirtualFolder(label, type, path.trim());
      setName("");
      setPath("");
      await onAdded();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="library-picker">
      <div className="picker-row">
        <label className="field">
          <span>Type</span>
          <select value={type} onChange={(e) => setType(e.target.value as LibraryType)}>
            {LIBRARY_TYPES.map((t) => (
              <option key={t.value} value={t.value}>
                {t.icon}  {t.label}
              </option>
            ))}
          </select>
        </label>

        <label className="field grow">
          <span>Name</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Defaults to the folder name"
          />
        </label>
      </div>

      <label className="field">
        <span>Folder on this server</span>
        <input
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="/mnt/media/movies"
        />
      </label>

      <details className="browser">
        <summary onClick={() => void browse(browsePath)}>
          Browse the server&rsquo;s folders
        </summary>
        <div className="browser-body">
          <p className="muted small path">{browsePath}</p>
          <div className="browser-list">
            {browsing && <p className="muted">Loading…</p>}
            {!browsing && entries.length === 0 && <p className="muted">No subfolders.</p>}
            {entries.map((entry) => (
              <button
                key={entry.path}
                type="button"
                className="link"
                onClick={() => {
                  if (entry.name === "..") void browse(entry.path);
                  else {
                    setPath(entry.path);
                    void browse(entry.path);
                  }
                }}
              >
                {entry.name === ".." ? "↑ .." : `${entry.name}/`}
              </button>
            ))}
          </div>
        </div>
      </details>

      {error && <p className="error">{error}</p>}

      <button type="button" className="primary" onClick={() => void add()} disabled={busy}>
        {busy ? "Adding…" : "Add library"}
      </button>
    </div>
  );
}