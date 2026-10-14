/**
 * Mounts the real App against a fake server.
 *
 * These exist because the client had never actually been executed: typechecking
 * proves the shapes line up, not that a screen renders. Each test here failed
 * on first run.
 */

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { App } from "../src/App";
import { api } from "../src/api/client";
import { mockServer, seedAdmin, resetStorage, setStoredToken, ADMIN_USER, type FakeServer } from "./setup";

let server: FakeServer;

beforeEach(async () => {
  server = mockServer();
  resetStorage();
  // Awaited: `logout` clears the client's module-level token, and if it
  // resolved late it would wipe the token the *next* test just primed, which
  // made tests pass alone and fail in a suite.
  await api.logout().catch(() => {});
});

afterEach(() => {
  server.restore();
  window.history.replaceState({}, "", "/");
});

describe("App boot", () => {
  it("shows a loading state, then the login form when signed out", async () => {
    seedAdmin(server);
    resetStorage();

    render(<App />);

    // The spinner is transient but must appear first.
    await waitFor(() => expect(screen.getByRole("button", { name: /sign in/i })).toBeInTheDocument());
    expect(screen.getByText("Test Server")).toBeInTheDocument();
  });

  it("restores a stored session straight to the dashboard", async () => {
    seedAdmin(server);
    // Simulate a reload with a token already in localStorage.
    setStoredToken("stored-token");

    render(<App />);

    // "My Libraries" is the dashboard heading; if this appears, the session
    // was restored rather than bounced to login.
    await waitFor(() => expect(screen.getByText("My Libraries")).toBeInTheDocument(), {
      timeout: 4000,
    });
    const tileGrid = document.querySelector(".grid.views");
    expect(tileGrid).not.toBeNull();
    expect(within(tileGrid!).getByRole("button", { name: /films/i })).toBeInTheDocument();
  });

  it("drops an invalid stored token and shows login", async () => {
    seedAdmin(server);
    server.on("GET", "/Users/Me", { Message: "Invalid token" }, 401);
    setStoredToken("stale-token");

    render(<App />);

    await waitFor(() => expect(screen.getByRole("button", { name: /sign in/i })).toBeInTheDocument());
  });

  it("shows the wizard when setup has not been completed", async () => {
    server.on("GET", "/System/Info/Public", {
      ServerName: "New Server",
      Version: "0.1.0",
      Id: "server-1",
      StartupWizardCompleted: false,
    });
    server.on("GET", "/Startup/FirstUser", null);

    render(<App />);

    await waitFor(() =>
      expect(screen.getByText(/welcome to jellyfin-clone/i)).toBeInTheDocument(),
    );
  });

  it("reports an unreachable server instead of a blank screen", async () => {
    globalThis.fetch = (async () => {
      throw new TypeError("Failed to fetch");
    }) as typeof fetch;

    render(<App />);

    // Match the heading specifically: the detail line repeats the phrase.
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: /cannot reach the server/i }),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText(/reload this page/i)).toBeInTheDocument();
  });
});

describe("sign in", () => {
  it("authenticates and lands on the dashboard", async () => {
    seedAdmin(server);
    server.on("POST", "/Users/AuthenticateByName", {
      User: ADMIN_USER,
      AccessToken: "new-token",
      ServerId: "server-1",
    });

    render(<App />);
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText(/username/i), "huzaim");
    await user.type(screen.getByLabelText(/password/i), "secret123");
    await user.click(screen.getByRole("button", { name: /^sign in$/i }));

    await waitFor(() => expect(screen.getByText("My Libraries")).toBeInTheDocument());
    expect(localStorage.getItem("jf.token")).toBe("new-token");
  });

  it("shows the server error and stays on the form for bad credentials", async () => {
    seedAdmin(server);
    server.on("POST", "/Users/AuthenticateByName", { Message: "Invalid username or password." }, 401);

    render(<App />);
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText(/username/i), "wrong");
    await user.click(screen.getByRole("button", { name: /^sign in$/i }));

    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(/invalid username or password/i),
    );
    // Must not have navigated away.
    expect(screen.getByLabelText(/username/i)).toBeInTheDocument();
  });
});

describe("dashboard", () => {
  beforeEach(() => {
    setStoredToken("stored-token");
    seedAdmin(server);
  });

  it("renders library tiles from the server", async () => {
    render(<App />);

    await waitFor(() => expect(screen.getByText("My Libraries")).toBeInTheDocument());
    const tileGrid = document.querySelector(".grid.views");
    expect(tileGrid).not.toBeNull();
    expect(within(tileGrid!).getByRole("button", { name: /films/i })).toBeInTheDocument();
  });

  it("collapses the home sidebar behind a hamburger menu", async () => {
    render(<App />);
    const user = userEvent.setup();

    const menu = await screen.findByRole("button", { name: /open navigation menu/i });
    expect(screen.queryByRole("button", { name: "Browse" })).not.toBeInTheDocument();

    await user.click(menu);
    expect(screen.getByRole("button", { name: "Browse" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /close navigation menu/i })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /close navigation menu/i }));
    expect(screen.queryByRole("button", { name: "Browse" })).not.toBeInTheDocument();
  });

  it.each([
    ["Restart", "/System/Restart", "restart"],
    ["Shutdown", "/System/Shutdown", "shut down"],
  ] as const)("allows an administrator to %s the server", async (label, endpoint, verb) => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<App />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /open navigation menu/i }));
    await user.click(screen.getByRole("button", { name: /dashboard/i }));
    await screen.findByRole("heading", { name: /^dashboard$/i });
    await user.click(screen.getByRole("button", { name: label }));

    expect(confirm).toHaveBeenCalledWith(expect.stringContaining(verb));
    await waitFor(() => {
      expect(server.calls.some((call) => call.method === "POST" && call.path === endpoint)).toBe(true);
    });
    expect(await screen.findByRole("status")).toHaveTextContent(
      label === "Restart" ? /restarting the server/i : /server is shutting down/i,
    );
  });

  it("shows a progress panel instead of tiles while scanning", async () => {
    server.on("GET", "/Library/ScanStatus", {
      scanning: true,
      libraries: {
        "lib-1": { state: "Scanning", progress: 0.5, processed: 10, detail: null },
      },
    });

    render(<App />);

    await waitFor(() => expect(screen.getByText(/scanning your libraries/i)).toBeInTheDocument());
    // Tiles are replaced, not shown alongside.
    expect(screen.queryByText("My Libraries")).not.toBeInTheDocument();
    expect(screen.getByText("50%")).toBeInTheDocument();
  });

  it("explains when a scan found no supported media files", async () => {
    server.on("GET", "/Library/ScanStatus", {
      scanning: false,
      libraries: {
        "lib-1": {
          state: "Done",
          progress: 1,
          processed: 0,
          detail: "No supported media files found. Check the folder and its file formats.",
        },
      },
    });

    render(<App />);

    expect(
      await screen.findByText(/no supported media files found/i),
    ).toBeInTheDocument();
  });

  it("explains how to add a library when there are none", async () => {
    server.on("GET", "/Library/VirtualFolders", []);
    server.on("GET", `/Users/${ADMIN_USER.Id}/Views`, {
      Items: [],
      TotalRecordCount: 0,
      StartIndex: 0,
    });

    render(<App />);

    await waitFor(() => expect(screen.getByText(/no libraries yet/i)).toBeInTheDocument());
  });

  it("surfaces a library load failure rather than rendering empty", async () => {
    server.on("GET", "/Library/VirtualFolders", { Message: "boom" }, 500);

    render(<App />);

    await waitFor(() => expect(screen.getByText("boom")).toBeInTheDocument());
  });

  it("renders resume items with their progress", async () => {
    server.on("GET", `/Users/${ADMIN_USER.Id}/Items/Resume`, {
      Items: [
        {
          Id: "m1",
          Name: "Dune",
          Type: "Movie",
          ServerId: "server-1",
          IsFolder: false,
          ProductionYear: 2021,
          RunTimeTicks: 6 * 3600 * 10_000_000,
          ImageTags: { Primary: "tag" },
          UserData: { PlaybackPositionTicks: 1 * 3600 * 10_000_000, PlayCount: 0, IsFavorite: false, Played: false, UnplayedItemCount: 0 },
        },
      ],
      TotalRecordCount: 1,
      StartIndex: 0,
    });

    render(<App />);

    await waitFor(() => expect(screen.getByText("Continue Watching")).toBeInTheDocument());
    expect(screen.getByText("Dune")).toBeInTheDocument();
    // One hour into a six-hour film: five hours remain, which is what
    // Jellyfin labels the tile with.
    expect(screen.getByText("5h 0m left")).toBeInTheDocument();
  });
});