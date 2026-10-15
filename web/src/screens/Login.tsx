import { useEffect, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import type { PublicUser } from "../types";

/**
 * Sign-in screen.
 *
 * When the server has passwordless accounts, their names are offered as
 * one-click buttons, which is the common case on a home server.
 */
export function Login({ serverName }: { serverName: string }) {
  const { login } = useSession();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [users, setUsers] = useState<PublicUser[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api
      .publicUsers()
      .then((data) => setUsers(data.Users))
      .catch(() => setUsers([]));
  }, []);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await login(username, password);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function quickSignIn(user: PublicUser) {
    setUsername(user.Name);
    setPassword("");
    setBusy(true);
    setError("");
    try {
      // A passwordless account signs in with an empty password.
      await login(user.Name, "");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="centered">
      <form className="card login" onSubmit={submit}>
        <div className="login-brand">
          <span className="brand-mark" aria-hidden="true">
            <svg viewBox="0 0 28 28">
              <path d="M5 8.5 14 3l9 5.5v11L14 25l-9-5.5z" />
              <path d="m11 9 7 5-7 5z" />
            </svg>
          </span>
          <strong>{serverName}</strong>
        </div>

        <h1>Welcome back</h1>
        <p className="muted">Sign in to keep watching.</p>

        <label htmlFor="username">Username</label>
        <input
          id="username"
          name="username"
          autoComplete="username"
          autoFocus
          required
          value={username}
          onChange={(e) => setUsername(e.target.value)}
        />

        <label htmlFor="password">Password</label>
        <input
          id="password"
          name="password"
          type="password"
          autoComplete="current-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />

        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}

        <button className="primary" type="submit" disabled={busy}>
          {busy ? "Signing in…" : "Sign in"}
        </button>

        {users.filter((u) => !u.HasPassword).length > 0 && (
          <>
            <div className="divider" />
            <p className="muted small">Or sign in as</p>
            <div className="quick-users">
              {users
                .filter((u) => !u.HasPassword)
                .map((u) => (
                  <button key={u.Id} type="button" className="ghost" onClick={() => quickSignIn(u)}>
                    {u.Name}
                  </button>
                ))}
            </div>
          </>
        )}
      </form>
    </div>
  );
}