/**
 * Theme system.
 *
 * The active scheme is one `data-theme` attribute on <html> plus CSS variables,
 * so these assert the attribute/storage/event contract rather than colours —
 * the palette itself is CSS and is covered by the browser, not jsdom.
 */

import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { App } from "../src/App";
import { api } from "../src/api/client";
import {
  DEFAULT_THEME,
  THEMES,
  applyTheme,
  getStoredTheme,
  setTheme,
  subscribeTheme,
} from "../src/theme";
import { mockServer, seedAdmin, resetStorage, setStoredToken, type FakeServer } from "./setup";

let server: FakeServer;

beforeEach(async () => {
  server = mockServer();
  resetStorage();
  await api.logout().catch(() => {});
});

afterEach(() => {
  server.restore();
  delete document.documentElement.dataset.theme;
  window.history.replaceState({}, "", "/");
});

describe("theme manager", () => {
  it("exposes Jellyfin's schemes and defaults to dark", () => {
    expect(DEFAULT_THEME).toBe("dark");
    expect(THEMES.map((theme) => theme.id)).toEqual([
      "dark",
      "light",
      "appletv",
      "blueradiance",
      "purplehaze",
      "wmc",
      "cinema",
    ]);
  });

  it("applies a scheme to the document and its colour-scheme hint", () => {
    applyTheme("light");

    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.querySelector('meta[name="color-scheme"]')?.getAttribute("content")).toBe(
      "light",
    );

    applyTheme("dark");
    expect(document.querySelector('meta[name="color-scheme"]')?.getAttribute("content")).toBe(
      "dark",
    );
  });

  it("persists a chosen theme and notifies subscribers", () => {
    const seen: string[] = [];
    const unsubscribe = subscribeTheme((id) => seen.push(id));

    setTheme("purplehaze");

    expect(localStorage.getItem("jf.theme")).toBe("purplehaze");
    expect(document.documentElement.dataset.theme).toBe("purplehaze");
    expect(seen).toEqual(["purplehaze"]);

    unsubscribe();
    setTheme("wmc");
    expect(seen).toEqual(["purplehaze"]);
  });

  it("ignores an unknown stored theme rather than breaking the palette", () => {
    localStorage.setItem("jf.theme", "not-a-theme");
    expect(getStoredTheme()).toBe(DEFAULT_THEME);
  });
});

describe("theme picker", () => {
  beforeEach(() => {
    setStoredToken("stored-token");
    seedAdmin(server);
  });

  it("switches the document theme from the account menu", async () => {
    render(<App />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /account menu/i }));
    const picker = screen.getByLabelText(/theme/i);

    // The seeded session has no stored theme, so it starts on the default.
    expect(document.documentElement.dataset.theme).toBe("dark");

    await user.selectOptions(picker, "light");

    expect(document.documentElement.dataset.theme).toBe("light");
    expect(localStorage.getItem("jf.theme")).toBe("light");
  });

  it("does not query the server to change a theme", async () => {
    render(<App />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /account menu/i }));
    const before = server.calls.length;
    await user.selectOptions(screen.getByLabelText(/theme/i), "appletv");

    expect(server.calls.length).toBe(before);
  });
});
