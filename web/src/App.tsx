import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { api, ApiError } from "./api/client";
import type { PublicSystemInfo, User } from "./types";
import { useRoute, type Route } from "./router";
import { TopBar } from "./components/TopBar";
import { Login } from "./screens/Login";
import { Home } from "./screens/Home";
import { Welcome } from "./screens/Welcome";
import { LibrarySettings } from "./screens/Dashboard";
import { Library } from "./screens/Library";
import { ItemDetail } from "./screens/ItemDetail";
import { Player } from "./screens/Player";

type Status = "loading" | "onboarding" | "unauthenticated" | "ready" | "unreachable";

interface Session {
  user: User;
  server: PublicSystemInfo;
}

interface AppContextValue {
  session: Session | null;
  status: Status;
  error: string;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
}

const AppContext = createContext<AppContextValue | null>(null);

export function useSession(): AppContextValue {
  const value = useContext(AppContext);
  if (!value) throw new Error("useSession must be used inside <App>");
  return value;
}

/**
 * Application shell: owns the session and picks the screen for the current
 * route.
 */
export function App() {
  const [status, setStatus] = useState<Status>("loading");
  const [session, setSession] = useState<Session | null>(null);
  const [error, setError] = useState("");
  const [route, go] = useRoute();
  const [playing, setPlaying] = useState<{ itemId: string; title: string } | null>(null);

  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        const server = await api.publicInfo();
        if (cancelled) return;

        if (await api.restore()) {
          const user = await api.me();
          if (cancelled) return;
          setSession({ user, server });
          setStatus("ready");
        } else if (server.StartupWizardCompleted) {
          setStatus("unauthenticated");
        } else {
          // No accounts and the wizard is unfinished: send the user through
          // onboarding rather than to a login form they cannot pass.
          setStatus("onboarding");
        }
      } catch (err) {
        if (cancelled) return;
        // A server that is down is a different state from being signed out:
        // showing the login form would imply credentials are the problem.
        const message = err instanceof ApiError ? err.message : String(err);
        setError(`Cannot reach the server: ${message}`);
        setStatus("unreachable");
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  const login = useCallback(async (username: string, password: string) => {
    setError("");
    try {
      const result = await api.login(username, password);
      const server = await api.publicInfo().catch(() => ({
        ServerName: "jellyfin-clone",
        Version: "",
        Id: "",
        StartupWizardCompleted: true,
      }));
      setSession({ user: result.User, server });
      setStatus("ready");
    } catch (err) {
      const message = err instanceof ApiError ? err.message : String(err);
      setError(message);
      throw err;
    }
  }, []);

  const logout = useCallback(async () => {
    await api.logout();
    setSession(null);
    setStatus("unauthenticated");
    go({ name: "home" }, true);
  }, [go]);

  const context = useMemo<AppContextValue>(
    () => ({ session, status, error, login, logout }),
    [session, status, error, login, logout],
  );

  if (status === "loading") {
    return (
      <div className="centered">
        <div className="spinner" aria-label="Loading" />
      </div>
    );
  }

  if (status === "unreachable") {
    return (
      <div className="centered">
        <div className="card panel">
          <h1>Cannot reach the server</h1>
          <p className="muted">{error}</p>
          <p className="muted">
            Check that <code>jellyfin-clone</code> is running and reload this page.
          </p>
        </div>
      </div>
    );
  }

  if (status === "onboarding") {
    // The wizard creates the first account itself, so it runs outside the
    // session context entirely.
    return (
      <AppContext.Provider value={context}>
        <Welcome go={go} />
      </AppContext.Provider>
    );
  }

  if (status === "unauthenticated" || !session) {
    return (
      <AppContext.Provider value={context}>
        <Login serverName={session?.server.ServerName ?? "jellyfin-clone"} />
      </AppContext.Provider>
    );
  }

  return (
    <AppContext.Provider value={context}>
      <TopBar />
      {playing ? (
        <Player
          itemId={playing.itemId}
          title={playing.title}
          onClose={() => {
            setPlaying(null);
            // Returning to the detail page keeps the back button sensible.
            go({ name: "item", itemId: playing.itemId }, true);
          }}
        />
      ) : (
        <Screen route={route} go={go} onPlay={setPlaying} />
      )}
    </AppContext.Provider>
  );
}

interface ScreenProps {
  route: Route;
  go: (route: Route, replace?: boolean) => void;
  onPlay: (item: { itemId: string; title: string }) => void;
}

function Screen({ route, go, onPlay }: ScreenProps) {
  switch (route.name) {
    case "home":
      return <Home go={go} onPlay={onPlay} />;
    case "dashboard":
      return <LibrarySettings go={go} />;
    case "library":
      return <Library viewId={route.viewId} go={go} onPlay={onPlay} />;
    case "item":
      return <ItemDetail itemId={route.itemId} go={go} onPlay={onPlay} />;
    case "search":
      return <Library viewId={null} search={route.term} go={go} onPlay={onPlay} />;
  }
}