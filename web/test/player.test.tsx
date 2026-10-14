import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { createElement, type PropsWithChildren, type VideoHTMLAttributes } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { App } from "../src/App";
import { api } from "../src/api/client";
import { mockServer, seedAdmin, resetStorage, setStoredToken, type FakeServer } from "./setup";

vi.mock("@videojs/react/video", () => ({
  VideoPlayer: ({ children }: PropsWithChildren) =>
    createElement("div", { "data-testid": "videojs-player" }, children),
  VideoSkin: ({ children, className }: PropsWithChildren<{ className?: string }>) =>
    createElement("div", { className }, children),
  Video: (props: VideoHTMLAttributes<HTMLVideoElement>) =>
    createElement("video", { ...props, "data-testid": "video-element" }),
}));

let server: FakeServer;

beforeEach(async () => {
  server = mockServer();
  resetStorage();
  await api.logout().catch(() => {});
  setStoredToken("stored-token");
  window.history.replaceState({}, "", "/item/movie-1");
});

afterEach(() => {
  server.restore();
  window.history.replaceState({}, "", "/");
});

describe("fullscreen player", () => {
  it("replaces the app chrome with an edge-to-edge player and can close it", async () => {
    seedAdmin(server);
    server.on("GET", "/Users/user-1/Items/movie-1", {
      Id: "movie-1",
      Name: "A Film",
      Type: "Movie",
      ServerId: "server-1",
      IsFolder: false,
      RunTimeTicks: 600 * 10_000_000,
      UserData: {
        PlaybackPositionTicks: 0,
        PlayCount: 0,
        IsFavorite: false,
        Played: false,
        UnplayedItemCount: 0,
      },
    });

    render(<App />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /^play$/i }));

    expect(await screen.findByTestId("videojs-player")).toBeInTheDocument();
    expect(screen.getByTestId("video-element")).toHaveAttribute(
      "src",
      "/Videos/movie-1/stream?static=true&api_key=stored-token",
    );
    expect(document.querySelector(".player-stage")).toBeInTheDocument();
    expect(screen.queryByRole("banner")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /close player/i }));
    await waitFor(() => expect(screen.getByRole("banner")).toBeInTheDocument());
  });
});
