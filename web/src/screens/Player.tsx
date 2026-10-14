import { useEffect, useRef, useState } from "react";
import { Video, VideoPlayer, VideoSkin } from "@videojs/react/video";
import "@videojs/react/video/skin.css";
import { useSession } from "../App";
import { api } from "../api/client";

export function Player({
  itemId,
  title,
  onClose,
}: {
  itemId: string;
  title: string;
  onClose: () => void;
}) {
  const { session } = useSession();
  const userId = session?.user.Id ?? "";
  const [startPosition, setStartPosition] = useState<number | null>(null);
  const [error, setError] = useState("");
  const positionRef = useRef(0);
  const startedRef = useRef(false);
  const pausedRef = useRef(false);
  const playbackStartedRef = useRef(false);

  useEffect(() => {
    let cancelled = false;

    void api
      .item(userId, itemId)
      .then((item) => {
        const resume = item.UserData?.PlaybackPositionTicks ?? 0;
        const runtime = item.RunTimeTicks ?? 0;
        const start = runtime && resume >= runtime * 0.98 ? 0 : resume / 10_000_000;
        if (!cancelled) {
          positionRef.current = start;
          setStartPosition(start);
        }
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : String(err));
        setStartPosition(0);
      });

    return () => {
      cancelled = true;
    };
  }, [itemId, userId]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (!startedRef.current || pausedRef.current) return;
      void api
        .reportProgress(itemId, Math.round(positionRef.current * 10_000_000), false)
        .catch(() => {});
    }, 15_000);
    return () => window.clearInterval(timer);
  }, [itemId]);

  useEffect(() => {
    return () => {
      if (!startedRef.current) return;
      void api
        .reportStopped(itemId, Math.round(positionRef.current * 10_000_000))
        .catch(() => {});
    };
  }, [itemId]);

  return (
    <div className="player-stage">
      <button
        type="button"
        className="player-close"
        aria-label="Close player"
        title="Close player"
        onClick={onClose}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="m6 6 12 12M18 6 6 18" />
        </svg>
      </button>

      {startPosition === null ? (
        <div className="player-loading" role="status" aria-label="Loading video">
          <span className="spinner" />
        </div>
      ) : (
        <VideoPlayer title={title}>
          <VideoSkin className="jelly-player-skin">
            <Video
              className="jelly-player-video"
              src={api.streamUrl(itemId)}
              preload="auto"
              autoPlay
              playsInline
              onLoadedMetadata={(event) => {
                const video = event.currentTarget;
                video.currentTime = startPosition;
                positionRef.current = startPosition;
              }}
              onTimeUpdate={(event) => {
                positionRef.current = event.currentTarget.currentTime;
              }}
              onPlay={() => {
                startedRef.current = true;
                pausedRef.current = false;
                if (playbackStartedRef.current) return;
                playbackStartedRef.current = true;
                void api
                  .reportPlaybackStart(
                    itemId,
                    Math.round(positionRef.current * 10_000_000),
                    `jf-${itemId.slice(0, 8)}`,
                  )
                  .catch(() => {});
              }}
              onPause={() => {
                pausedRef.current = true;
              }}
              onError={() => setError("This item could not be played.")}
            />
          </VideoSkin>
        </VideoPlayer>
      )}

      {error && <p className="player-error">{error}</p>}
    </div>
  );
}
