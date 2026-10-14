/**
 * Router, and the wiring between components and it.
 *
 * Every screen that used to be unreachable — the dashboard in particular —
 * got past typechecking and built cleanly. These tests exist to catch that
 * class of bug: a navigation call that updates the URL but not the screen.
 */

import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { App } from "../src/App";
import { hrefFor, navigate, parseLocation } from "../src/router";
import { api } from "../src/api/client";
import { mockServer, seedAdmin, resetStorage, setStoredToken, type FakeServer } from "./setup";

let server: FakeServer;

beforeEach(async () => {
  server = mockServer();
  resetStorage();
  await api.logout().catch(() => {});
  window.history.replaceState({}, "", "/");
});

afterEach(() => {
  server.restore();
  window.history.replaceState({}, "", "/");
});

describe("route parsing", () => {
  it("round-trips every route through its href", () => {
    const routes = [
      { name: "home" },
      { name: "dashboard" },
      { name: "library", viewId: "lib-1" },
      { name: "item", itemId: "item-1" },
      { name: "search", term: "dune" },
    ] as const;

    for (const route of routes) {
      const href = hrefFor(route);
      const [pathname, search = ""] = href.split("?");
      expect(parseLocation(pathname!, search)).toEqual(route);
    }
  });

  it("encodes search terms that contain URL-significant characters", () => {
    const href = hrefFor({ name: "search", term: "a & b/c" });
    expect(href).toBe("/search?q=a%20%26%20b%2Fc");
    const [pathname, search = ""] = href.split("?");
    expect(parseLocation(pathname!, search)).toEqual({ name: "search", term: "a & b/c" });
  });

  it("falls back to home for an unknown path rather than throwing", () => {
    expect(parseLocation("/nonsense/deep/path", "")).toEqual({ name: "home" });
  });
});

describe("in-app navigation", () => {
  beforeEach(() => {
    setStoredToken("stored-token");
  });

  it("changes the screen, not just the URL", async () => {
    seedAdmin(server);
    render(<App />);
    const user = userEvent.setup();

    await screen.findByText("My Libraries");
    expect(window.location.pathname).toBe("/");

    // The regression this guards: the URL changed but the screen did not.
    await user.click(screen.getByRole("button", { name: /open navigation menu/i }));
    await user.click(screen.getByRole("button", { name: /dashboard/i }));

    await waitFor(() =>
      expect(screen.getByRole("heading", { name: /^dashboard$/i })).toBeInTheDocument(),
    );
    expect(window.location.pathname).toBe("/dashboard");
  });

  it("returns home from the dashboard", async () => {
    seedAdmin(server);
    render(<App />);
    const user = userEvent.setup();

    await screen.findByText("My Libraries");
    await user.click(screen.getByRole("button", { name: /open navigation menu/i }));
    await user.click(screen.getByRole("button", { name: /dashboard/i }));
    await screen.findByRole("heading", { name: /^dashboard$/i });

    await user.click(screen.getByRole("button", { name: /← back/i }));

    await waitFor(() => expect(screen.getByText("My Libraries")).toBeInTheDocument());
    expect(window.location.pathname).toBe("/");
  });

  it("opens a library tile on that library's page", async () => {
    seedAdmin(server);
    server.on("GET", "/Users/user-1/Items/root-1", {
      Items: [
        {
          Id: "movie-1",
          Name: "Example Movie",
          Type: "Movie",
          ServerId: "server-1",
          IsFolder: false,
        },
      ],
      TotalRecordCount: 1,
      StartIndex: 0,
    });
    render(<App />);
    const user = userEvent.setup();

    await screen.findByText("My Libraries");
    const tileGrid = document.querySelector(".grid.views");
    expect(tileGrid).not.toBeNull();
    await user.click(within(tileGrid!).getByRole("button", { name: /films/i }));

    expect(await screen.findByText("Example Movie")).toBeInTheDocument();
    expect(window.location.pathname).toBe("/library/root-1");
    expect(screen.queryByRole("button", { name: /open navigation menu/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: /main navigation/i })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /libraries/i }));
    await screen.findByRole("heading", { name: /my libraries/i });
    expect(window.location.pathname).toBe("/");
  });

  it("follows a browser back button", async () => {
    seedAdmin(server);
    render(<App />);
    const user = userEvent.setup();

    await screen.findByText("My Libraries");
    await user.click(screen.getByRole("button", { name: /open navigation menu/i }));
    await user.click(screen.getByRole("button", { name: /dashboard/i }));
    await screen.findByRole("heading", { name: /^dashboard$/i });

    window.history.replaceState({}, "", "/");
    window.dispatchEvent(new PopStateEvent("popstate"));

    await waitFor(() => expect(screen.getByText("My Libraries")).toBeInTheDocument());
  });

  it("renders the dashboard directly when the URL is already /dashboard", async () => {
    seedAdmin(server);
    window.history.replaceState({}, "", "/dashboard");

    render(<App />);

    expect(await screen.findByRole("heading", { name: /^dashboard$/i })).toBeInTheDocument();
  });

  it("searches when the top-bar query is typed", async () => {
    seedAdmin(server);
    server.on("GET", "/Users/user-1/Items", {
      Items: [
        {
          Id: "m1",
          Name: "Dune",
          Type: "Movie",
          ServerId: "server-1",
          IsFolder: false,
          ProductionYear: 2021,
        },
      ],
      TotalRecordCount: 1,
      StartIndex: 0,
    });

    render(<App />);
    const user = userEvent.setup();
    await screen.findByText("My Libraries");

    await user.type(screen.getByRole("searchbox"), "dune");

    await waitFor(() => expect(screen.getByText("Dune")).toBeInTheDocument(), { timeout: 3000 });
  });
});

describe("route module", () => {
  it("notifies subscribers on in-app navigation", () => {
    let seen = 0;
    const listener = () => seen++;
    window.addEventListener("jf:navigate", listener);

    navigate({ name: "dashboard" });
    window.removeEventListener("jf:navigate", listener);

    expect(seen).toBe(1);
  });
});
