/**
 * Multi-user administration.
 *
 * The point of these is that the wire shapes are exact: creating an account
 * must reach `POST /Users/New`, deleting must reach `DELETE /Users/{id}`, and —
 * the subtle one — saving access must post the *whole* policy, because the
 * server replaces the object and rejects a partial body. A test that only
 * checked "a request happened" would miss a policy silently losing its
 * unmodelled fields.
 */

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
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

/** A non-administrator, to exercise access checks. */
const VIEWER = {
  ...ADMIN_USER,
  Id: "user-2",
  Name: "bob",
  HasPassword: false,
  Policy: {
    ...ADMIN_USER.Policy,
    IsAdministrator: false,
    EnableContentDeletion: false,
  },
};

beforeEach(async () => {
  server = mockServer();
  resetStorage();
  await api.logout().catch(() => {});
  window.history.replaceState({}, "", "/settings/users");
});

afterEach(() => {
  server.restore();
  window.history.replaceState({}, "", "/");
});

/** Requests of one kind, for assertions. */
function sent(server: FakeServer, method: string, path: string) {
  return server.calls.filter((c) => c.method === method && c.path === path);
}

/** The card element for one account, so per-user controls are unambiguous. */
async function cardFor(name: string): Promise<HTMLElement> {
  const label = await screen.findByText(name);
  const card = label.closest(".user-card");
  if (!card) throw new Error(`no user card for ${name}`);
  return card as HTMLElement;
}

function seedUsers() {
  setStoredToken("stored-token");
  seedAdmin(server);
  server.on("GET", "/Users", [ADMIN_USER, VIEWER]);
}

describe("user management", () => {
  it("lists accounts and creates a new user with the chosen role", async () => {
    seedUsers();
    render(<App />);
    const user = userEvent.setup();

    expect(await screen.findByRole("heading", { name: /^users$/i })).toBeInTheDocument();
    expect(await screen.findByText("bob")).toBeInTheDocument();
    expect(screen.getAllByText("huzaim").length).toBeGreaterThan(0);

    await user.type(screen.getByLabelText(/username/i), "carol");
    await user.type(screen.getByLabelText(/^password$/i), "s3cret");
    await user.selectOptions(screen.getByLabelText(/^role$/i), "admin");
    await user.click(screen.getByRole("button", { name: /add user/i }));

    await waitFor(() => expect(sent(server, "POST", "/Users/New")).toHaveLength(1));
    expect(sent(server, "POST", "/Users/New")[0]!.body).toEqual({
      Name: "carol",
      Password: "s3cret",
      IsAdministrator: true,
    });
  });

  it("changes a user's role and access, preserving the rest of the policy", async () => {
    seedUsers();
    render(<App />);
    const user = userEvent.setup();

    const card = await cardFor("bob");
    await user.click(within(card).getByRole("button", { name: /edit access for bob/i }));

    await user.selectOptions(within(card).getByLabelText(/^role$/i), "admin");
    await user.click(within(card).getByLabelText(/allow deleting media/i));
    await user.click(within(card).getByRole("button", { name: /save access/i }));

    await waitFor(() => expect(sent(server, "POST", "/Users/user-2/Policy")).toHaveLength(1));
    const body = sent(server, "POST", "/Users/user-2/Policy")[0]!.body as Record<string, unknown>;
    expect(body).toMatchObject({
      IsAdministrator: true,
      EnableContentDeletion: true,
    });
    // Unmodelled fields must survive: the server replaces the whole object.
    expect(body.EnableSyncTranscoding).toBe(true);
    expect(body.SyncPlayAccess).toBe("CreateAndJoinGroups");
  });

  it("grants access to specific libraries rather than all of them", async () => {
    seedUsers();
    render(<App />);
    const user = userEvent.setup();

    const card = await cardFor("bob");
    await user.click(within(card).getByRole("button", { name: /edit access for bob/i }));

    await user.click(within(card).getByLabelText(/access to all libraries/i));
    // Choosing no libraries at all would grant nothing, so pick the one shown.
    await user.click(await within(card).findByLabelText("Films"));
    await user.click(within(card).getByRole("button", { name: /save access/i }));

    await waitFor(() => expect(sent(server, "POST", "/Users/user-2/Policy")).toHaveLength(1));
    const body = sent(server, "POST", "/Users/user-2/Policy")[0]!.body as Record<string, unknown>;
    expect(body.EnableAllFolders).toBe(false);
    expect(body.EnabledFolders).toEqual(["lib-1"]);
  });

  it("deletes a user after confirmation", async () => {
    seedUsers();
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<App />);
    const user = userEvent.setup();

    const card = await cardFor("bob");
    await user.click(within(card).getByRole("button", { name: /delete bob/i }));

    expect(confirm).toHaveBeenCalled();
    await waitFor(() => expect(sent(server, "DELETE", "/Users/user-2")).toHaveLength(1));
  });

  it("does not delete when the confirmation is dismissed", async () => {
    seedUsers();
    vi.spyOn(window, "confirm").mockReturnValue(false);
    render(<App />);
    const user = userEvent.setup();

    const card = await cardFor("bob");
    await user.click(within(card).getByRole("button", { name: /delete bob/i }));

    expect(sent(server, "DELETE", "/Users/user-2")).toHaveLength(0);
  });

  it("resets a user's password", async () => {
    seedUsers();
    render(<App />);
    const user = userEvent.setup();

    const card = await cardFor("bob");
    await user.click(within(card).getByRole("button", { name: /reset password for bob/i }));
    await user.type(within(card).getByLabelText(/new password/i), "letmein");
    await user.click(within(card).getByRole("button", { name: /^set password$/i }));

    await waitFor(() => expect(sent(server, "POST", "/Users/user-2/Password")).toHaveLength(1));
    expect(sent(server, "POST", "/Users/user-2/Password")[0]!.body).toEqual({
      NewPw: "letmein",
      ResetPassword: false,
    });
  });

  it("refuses the screen to a non-administrator", async () => {
    setStoredToken("stored-token");
    seedAdmin(server);
    server.on("GET", "/Users/Me", VIEWER);
    server.on("GET", "/Users/Public", {
      ServerId: "server-1",
      ServerName: "Test Server",
      Users: [VIEWER],
    });

    render(<App />);

    expect(
      await screen.findByText(/user management requires administrator access/i),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /add user/i })).not.toBeInTheDocument();
  });
});
