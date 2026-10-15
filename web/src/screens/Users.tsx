import { useCallback, useEffect, useState, type FormEvent, type ReactNode } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { useLibraries, type VirtualFolder } from "../library";
import type { User, UserPolicy } from "../types";
import type { Route } from "../router";

/**
 * Multi-user administration: who can use the server, and what each account
 * may reach.
 *
 * Admin-only in the UI, and the server enforces it too — a hidden route is not
 * the security boundary, so every request still carries the token and the
 * server decides.
 *
 * The role and access editor posts the *whole* policy: `POST /Users/{id}/Policy`
 * replaces the object and the server rejects a partial body, so the form starts
 * from the policy the server sent and overrides only the fields on screen.
 * Everything else survives untouched.
 */
export function Users({ go }: { go: (route: Route) => void }) {
  const { session } = useSession();
  const isAdmin = session?.user.Policy.IsAdministrator ?? false;
  // Library list, for the "specific libraries" access picker.
  const { folders } = useLibraries(isAdmin);
  const [users, setUsers] = useState<User[]>([]);
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState(false);

  const reload = useCallback(async () => {
    try {
      setUsers(await api.users());
      setError("");
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  if (!isAdmin || !session) {
    return (
      <main>
        <p className="error">User management requires administrator access.</p>
      </main>
    );
  }

  return (
    <main>
      <div className="section-head">
        <h1>Users</h1>
        <button type="button" className="link" onClick={() => go({ name: "home" })}>
          ← Back
        </button>
      </div>

      <div className="settings-page">
        <section className="api-settings-card">
          <span className="catalog-kicker">Access control</span>
          <h2>Add a user</h2>
          <p className="muted">
            Create an account, then tune exactly what it can reach.
          </p>
          <AddUserForm onCreated={reload} />
        </section>

        <section>
          <h2>Accounts</h2>

          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {!loaded && <p className="muted">Loading accounts…</p>}
          {loaded && users.length === 0 && (
            <p className="muted">No accounts yet. Add one above.</p>
          )}

          <div className="user-list">
            {users.map((user) => (
              <UserCard
                key={user.Id}
                user={user}
                folders={folders}
                isSelf={user.Id === session.user.Id}
                onChanged={reload}
              />
            ))}
          </div>
        </section>
      </div>
    </main>
  );
}

/** Create an account, choosing its role up front. */
function AddUserForm({ onCreated }: { onCreated: () => Promise<void> }) {
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [isAdmin, setIsAdmin] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.createUser(name.trim(), password, isAdmin);
      setName("");
      setPassword("");
      setIsAdmin(false);
      await onCreated();
    } catch (err) {
      setError(messageOf(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="user-form" onSubmit={submit}>
      <div className="user-form-fields">
        <label className="field">
          <span>Username</span>
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            autoComplete="off"
            required
          />
        </label>

        <label className="field">
          <span>Password</span>
          <input
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            autoComplete="new-password"
            placeholder="Leave blank for no password"
          />
        </label>

        <label className="field">
          <span>Role</span>
          <select
            value={isAdmin ? "admin" : "member"}
            onChange={(event) => setIsAdmin(event.target.value === "admin")}
          >
            <option value="member">Member</option>
            <option value="admin">Administrator</option>
          </select>
        </label>
      </div>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <div className="user-form-actions">
        <button type="submit" className="primary" disabled={busy || !name.trim()}>
          {busy ? "Creating…" : "Add user"}
        </button>
      </div>
    </form>
  );
}

/** One account, with its access editor and password reset behind a toggle. */
function UserCard({
  user,
  folders,
  isSelf,
  onChanged,
}: {
  user: User;
  folders: VirtualFolder[];
  isSelf: boolean;
  onChanged: () => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function remove() {
    if (!window.confirm(`Delete “${user.Name}”? This cannot be undone.`)) return;

    setBusy(true);
    setError("");
    try {
      await api.deleteUser(user.Id);
      await onChanged();
    } catch (err) {
      setError(messageOf(err));
      setBusy(false);
    }
  }

  return (
    <div className="user-card">
      <div className="user-row">
        <span className="user-avatar" aria-hidden="true">
          {(user.Name || "?").slice(0, 1).toUpperCase()}
        </span>

        <div className="user-info">
          <div className="user-name">
            {user.Name}
            {isSelf && <span className="chip">You</span>}
          </div>
          <div className="user-meta">
            <span className={`role-badge${user.Policy.IsAdministrator ? " admin" : ""}`}>
              {user.Policy.IsAdministrator ? "Administrator" : "Member"}
            </span>
            {user.Policy.IsDisabled && <span className="role-badge disabled">Disabled</span>}
            <span className="muted small">
              {user.HasPassword ? "Password set" : "No password"}
            </span>
          </div>
        </div>

        <div className="row-actions">
          <button
            type="button"
            className="ghost"
            aria-expanded={editing}
            aria-label={`Edit access for ${user.Name}`}
            onClick={() => {
              setEditing((open) => !open);
              setResetting(false);
            }}
          >
            Edit access
          </button>
          <button
            type="button"
            className="ghost"
            aria-expanded={resetting}
            aria-label={`Reset password for ${user.Name}`}
            onClick={() => {
              setResetting((open) => !open);
              setEditing(false);
            }}
          >
            Reset password
          </button>
          <button
            type="button"
            className="link danger"
            aria-label={`Delete ${user.Name}`}
            onClick={() => void remove()}
            disabled={busy}
          >
            Delete
          </button>
        </div>
      </div>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      {editing && (
        <AccessForm
          user={user}
          folders={folders}
          onCancel={() => setEditing(false)}
          onSaved={async () => {
            setEditing(false);
            await onChanged();
          }}
        />
      )}

      {resetting && (
        <ResetPasswordForm
          user={user}
          onCancel={() => setResetting(false)}
          onSaved={async () => {
            setResetting(false);
            await onChanged();
          }}
        />
      )}
    </div>
  );
}

/** Role and access editor for one account. */
function AccessForm({
  user,
  folders,
  onCancel,
  onSaved,
}: {
  user: User;
  folders: VirtualFolder[];
  onCancel: () => void;
  onSaved: () => Promise<void>;
}) {
  // Start from the server's policy so unmodelled fields survive the round-trip.
  const [policy, setPolicy] = useState<UserPolicy>(() => ({ ...user.Policy }));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  function update<K extends keyof UserPolicy>(key: K, value: UserPolicy[K]) {
    setPolicy((prev) => ({ ...prev, [key]: value }));
  }

  function toggleFolder(id: string, enabled: boolean) {
    setPolicy((prev) => ({
      ...prev,
      EnabledFolders: enabled
        ? [...prev.EnabledFolders.filter((f) => f !== id), id]
        : prev.EnabledFolders.filter((f) => f !== id),
    }));
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.updateUserPolicy(user.Id, policy);
      await onSaved();
    } catch (err) {
      setError(messageOf(err));
      setBusy(false);
    }
  }

  return (
    <form className="access-form" onSubmit={save}>
      <label className="field">
        <span>Role</span>
        <select
          value={policy.IsAdministrator ? "admin" : "member"}
          onChange={(event) => update("IsAdministrator", event.target.value === "admin")}
        >
          <option value="member">Member</option>
          <option value="admin">Administrator</option>
        </select>
      </label>

      <fieldset className="access-group">
        <legend>Permissions</legend>

        <CheckField
          label="Allow media playback"
          checked={policy.EnableMediaPlayback}
          onChange={(v) => update("EnableMediaPlayback", v)}
        />
        <CheckField
          label="Allow downloads"
          checked={policy.EnableContentDownloading}
          onChange={(v) => update("EnableContentDownloading", v)}
        />
        <CheckField
          label="Allow deleting media"
          checked={policy.EnableContentDeletion}
          onChange={(v) => update("EnableContentDeletion", v)}
        />
        <CheckField
          label="Disable this account"
          checked={policy.IsDisabled}
          onChange={(v) => update("IsDisabled", v)}
        />

        <label className="field inline-field">
          <span>Max concurrent sessions</span>
          <input
            type="number"
            min={0}
            value={policy.MaxActiveSessions ?? 0}
            onChange={(event) => update("MaxActiveSessions", Number(event.target.value) || 0)}
          />
        </label>
      </fieldset>

      <fieldset className="access-group">
        <legend>Library access</legend>

        <CheckField
          label="Access to all libraries"
          checked={policy.EnableAllFolders}
          onChange={(v) => update("EnableAllFolders", v)}
        />

        {!policy.EnableAllFolders &&
          (folders.length === 0 ? (
            <p className="muted small">No libraries to choose from yet.</p>
          ) : (
            <div className="access-libraries">
              {folders.map((folder) => (
                <CheckField
                  key={folder.ItemId}
                  label={folder.Name}
                  checked={policy.EnabledFolders.includes(folder.ItemId)}
                  onChange={(v) => toggleFolder(folder.ItemId, v)}
                />
              ))}
            </div>
          ))}
      </fieldset>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <div className="access-actions">
        <button type="submit" className="primary" disabled={busy}>
          {busy ? "Saving…" : "Save access"}
        </button>
        <button type="button" className="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/** Set or clear one account's password. */
function ResetPasswordForm({
  user,
  onCancel,
  onSaved,
}: {
  user: User;
  onCancel: () => void;
  onSaved: () => Promise<void>;
}) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.setUserPassword(user.Id, password);
      await onSaved();
    } catch (err) {
      setError(messageOf(err));
      setBusy(false);
    }
  }

  async function clear() {
    setBusy(true);
    setError("");
    try {
      await api.setUserPassword(user.Id, null);
      await onSaved();
    } catch (err) {
      setError(messageOf(err));
      setBusy(false);
    }
  }

  return (
    <form className="access-form" onSubmit={submit}>
      <label className="field">
        <span>New password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          required
        />
      </label>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <div className="access-actions">
        <button type="submit" className="primary" disabled={busy || !password}>
          {busy ? "Saving…" : "Set password"}
        </button>
        {user.HasPassword && (
          <button type="button" className="ghost" onClick={() => void clear()} disabled={busy}>
            Clear password
          </button>
        )}
        <button type="button" className="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/** A labelled checkbox; wrapping the input keeps the text the accessible name. */
function CheckField({
  label,
  checked,
  onChange,
}: {
  label: ReactNode;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <label className="checkbox-field">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>{label}</span>
    </label>
  );
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
