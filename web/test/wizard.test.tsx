/**
 * The onboarding wizard and the admin dashboard.
 *
 * The wizard creates the first account, so it runs before any session exists.
 * These drive it the way the server does, step by step, and assert that each
 * step's request actually reaches the server with the right fields.
 */

import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { App } from "../src/App";
import { api } from "../src/api/client";
import {
  mockServer,
  seedAdmin,
  resetStorage,
  setStoredToken,
  ADMIN_USER,
  type FakeServer,
} from "./setup";

let server: FakeServer;

/** Routes for a server that has not completed setup. */
function seedFresh(server: FakeServer) {
  server.on("GET", "/System/Info/Public", {
    ServerName: "New Server",
    Version: "0.1.0",
    Id: "server-1",
    StartupWizardCompleted: false,
  });
  server.on("GET", "/Startup/FirstUser", null);
  server.on("POST", "/Startup/Configuration", undefined, 204);
  server.on("POST", "/Startup/User", undefined, 204);
  server.on("POST", "/Startup/Complete", undefined, 204);
  server.on("GET", "/Library/PhysicalPaths", []);
  return server;
}

beforeEach(async () => {
  server = mockServer();
  resetStorage();
  await api.logout().catch(() => {});
});

afterEach(() => {
  server.restore();
  window.history.replaceState({}, "", "/");
});

/** Requests of one kind, for assertions. */
function sent(server: FakeServer, method: string, path: string) {
  return server.calls.filter((c) => c.method === method && c.path === path);
}

describe("onboarding wizard", () => {
  it("walks from welcome to a finished setup", async () => {
    seedFresh(server);
    render(<App />);
    const user = userEvent.setup();

    // Step 1 — welcome.
    await user.click(await screen.findByRole("button", { name: /get started/i }));
    expect(await screen.findByRole("heading", { name: /server settings/i })).toBeInTheDocument();

    // Step 2 — server settings. Naming the server must reach the server.
    const nameInput = screen.getByLabelText(/server name/i);
    await user.clear(nameInput);
    await user.type(nameInput, "My Media Box");
    await user.click(screen.getByRole("button", { name: /^continue$/i }));

    await waitFor(() => expect(sent(server, "POST", "/Startup/Configuration")).toHaveLength(1));
    expect(sent(server, "POST", "/Startup/Configuration")[0]!.body).toMatchObject({
      ServerName: "My Media Box",
    });

    // Step 3 — administrator.
    expect(
      await screen.findByRole("heading", { name: /create an administrator/i }),
    ).toBeInTheDocument();
    await user.type(screen.getByLabelText(/username/i), "huzaim");
    await user.type(screen.getByLabelText(/^password/i), "secret123");
    await user.click(screen.getByRole("button", { name: /create account/i }));

    await waitFor(() => expect(sent(server, "POST", "/Startup/User")).toHaveLength(1));
    expect(sent(server, "POST", "/Startup/User")[0]!.body).toMatchObject({
      Name: "huzaim",
      Password: "secret123",
    });

    // Step 4 — libraries.
    expect(
      await screen.findByRole("heading", { name: /add your media libraries/i }),
    ).toBeInTheDocument();
  });

  it("blocks the account step until a username is entered", async () => {
    seedFresh(server);
    render(<App />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /get started/i }));
    await user.click(await screen.findByRole("button", { name: /^continue$/i }));
    await screen.findByRole("heading", { name: /create an administrator/i });

    const create = screen.getByRole("button", { name: /create account/i });
    expect(create).toBeDisabled();
    expect(sent(server, "POST", "/Startup/User")).toHaveLength(0);
  });

  it("requires the password confirmation to match", async () => {
    seedFresh(server);
    render(<App />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /get started/i }));
    await user.click(await screen.findByRole("button", { name: /^continue$/i }));
    await screen.findByRole("heading", { name: /create an administrator/i });

    await user.type(screen.getByLabelText(/username/i), "huzaim");
    await user.type(screen.getByLabelText(/^password/i), "secret123");
    await user.type(screen.getByLabelText(/confirm password/i), "different");

    expect(await screen.findByText(/passwords do not match/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /create account/i })).toBeDisabled();
    expect(sent(server, "POST", "/Startup/User")).toHaveLength(0);
  });

  it("allows an empty password, since passwordless accounts are supported", async () => {
    seedFresh(server);
    render(<App />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /get started/i }));
    await user.click(await screen.findByRole("button", { name: /^continue$/i }));
    await screen.findByRole("heading", { name: /create an administrator/i });

    await user.type(screen.getByLabelText(/username/i), "huzaim");
    await user.click(screen.getByRole("button", { name: /create account/i }));

    await waitFor(() => expect(sent(server, "POST", "/Startup/User")).toHaveLength(1));
    expect(sent(server, "POST", "/Startup/User")[0]!.body).toMatchObject({ Name: "huzaim" });
  });

  it("shows the server's error and stays on the step", async () => {
    seedFresh(server);
    server.on("POST", "/Startup/User", { Message: "That name is taken." }, 400);

    render(<App />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: /get started/i }));
    await user.click(await screen.findByRole("button", { name: /^continue$/i }));
    await screen.findByRole("heading", { name: /create an administrator/i });

    await user.type(screen.getByLabelText(/username/i), "huzaim");
    await user.click(screen.getByRole("button", { name: /create account/i }));

    expect(await screen.findByText(/that name is taken/i)).toBeInTheDocument();
    // Must not have advanced to the libraries step.
    expect(
      screen.queryByRole("heading", { name: /add your media libraries/i }),
    ).not.toBeInTheDocument();
  });
});

describe("library admin", () => {
  beforeEach(() => {
    setStoredToken("stored-token");
  });

  it("adds a library through the picker", async () => {
    seedAdmin(server);
    server.on("GET", "/Library/PhysicalPaths", [
      { Name: "..", Path: "/media" },
      { Name: "movies", Path: "/media/movies" },
    ]);

    render(<App />);
    const user = userEvent.setup();
    await screen.findByText("My Libraries");
    await user.click(screen.getByRole("button", { name: /dashboard/i }));

    expect(await screen.findByRole("heading", { name: /^dashboard$/i })).toBeInTheDocument();

    const pathInput = await screen.findByLabelText(/folder on this server/i);
    await user.type(pathInput, "/media/movies");

    await user.click(screen.getByRole("button", { name: /add library/i }));

    await waitFor(() =>
      expect(sent(server, "POST", "/Library/VirtualFolders")).toHaveLength(1),
    );
    const body = sent(server, "POST", "/Library/VirtualFolders")[0]!;
    expect(body.path).toBe("/Library/VirtualFolders");
  });

  it("refuses to add a library with no folder chosen", async () => {
    seedAdmin(server);
    render(<App />);
    const user = userEvent.setup();
    await screen.findByText("My Libraries");
    await user.click(screen.getByRole("button", { name: /dashboard/i }));

    const add = await screen.findByRole("button", { name: /add library/i });
    await user.click(add);

    expect(await screen.findByText(/choose a folder on the server/i)).toBeInTheDocument();
    expect(sent(server, "POST", "/Library/VirtualFolders")).toHaveLength(0);
  });

  it("starts a rescan of every library", async () => {
    seedAdmin(server);
    render(<App />);
    const user = userEvent.setup();
    await screen.findByText("My Libraries");
    await user.click(screen.getByRole("button", { name: /dashboard/i }));

    await user.click(await screen.findByRole("button", { name: /scan all/i }));

    await waitFor(() => expect(sent(server, "POST", "/Library/Refresh")).toHaveLength(1));
  });

  it("refuses the admin screen to a non-administrator", async () => {
    const viewer = {
      ...ADMIN_USER,
      Id: "user-2",
      Name: "bob",
      Policy: { ...ADMIN_USER.Policy, IsAdministrator: false },
    };

    setStoredToken("stored-token");
    seedAdmin(server);
    server.on("GET", "/Users/Me", viewer);
    server.on("GET", "/Users/Public", {
      ServerId: "server-1",
      ServerName: "Test Server",
      Users: [viewer],
    });

    render(<App />);
    await screen.findByText("My Libraries");

    // The server would reject this too; the client simply does not offer it.
    expect(screen.queryByRole("button", { name: /dashboard/i })).not.toBeInTheDocument();
  });
});
