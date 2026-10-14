import { useCallback, useEffect, useRef, useState } from "react";
import { useSession } from "../App";
import { api } from "../api/client";
import { formatTime } from "../components/ItemCard";

/** How often playback position is reported to the server. */
const PROGRESS_INTERVAL_MS = 15_000;

/**
 * Video player.
 *
 * The `<video>` element is owned by React here rather than living outside the
 * tree: it appears only on the player screen, so React's normal lifecycle is
 * correct and there is no cross-tree coordination to get wrong.
 *
 * Position is reported on an interval rather than on every `timeupdate`, which
 * fires several times a second.
 */
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

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const startedRef = useRef(false);

  const [duration, setDuration] = useState(0);
  const [position, setPosition] = useState(0);
  const [buffered, setBuffered] = useState(0);
  const [paused, setPaused] = useState(false);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);
  const [error, setError] = useState("");

  // Latest known position, readable from the interval callback without
  // re-creating it on every timeupdate.
  const positionRef = useRef(0);
  positionRef.current = position;

  /** Seconds to resume from, for this user only. */
  async function resolveStartPosition(): Promise<number> {
    try {
      const item = await api.item(userId, itemId);
      const resume = item.UserData?.PlaybackPositionTicks ?? 0;
      const runtime = item.RunTimeTicks ?? 0;
      // Effectively finished: start over rather than at the credits.
      if (runtime && resume >= runtime * 0.98) return 0;
      return resume / 10_000_000;
    } catch {
      return 0;
    }
  }

  // Load and resume.
  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        const start = await resolveStartPosition();
        if (cancelled || !videoRef.current) return;

        videoRef.current.currentTime = start;
        setPosition(start);

        await api.reportPlaybackStart(itemId, Math.round(start * 10_000_000), `jf-${itemId.slice(0, 8)}`);
        await videoRef.current.play().catch(() => {
          // Autoplay can be blocked; the user can press play.
          setPaused(true);
        });
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      }
    })();

    return () => {
      cancelled = true;
    };
    // Intentionally keyed on itemId only: re-running on session changes would
    // restart playback mid-view.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [itemId]);

  // Report progress periodically.
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (paused || !videoRef.current) return;
      void api
        .reportProgress(itemId, Math.round(positionRef.current * 10_000_000), false)
        .catch(() => {
          // Best-effort: a dropped progress ping must not disturb playback.
        });
    }, PROGRESS_INTERVAL_MS);

    return () => window.clearInterval(timer);
  }, [itemId, paused]);

  // Report the final position when leaving.
  useEffect(() => {
    return () => {
      if (!startedRef.current) return;
      void api
        .reportStopped(itemId, Math.round(positionRef.current * 10_000_000))
        .catch(() => {});
    };
  }, [itemId]);

  const togglePlay = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) void video.play();
    else video.pause();
  }, []);

  function seekTo(event: React.MouseEvent<HTMLDivElement>) {
    const video = videoRef.current;
    if (!video || !duration) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const fraction = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    video.currentTime = fraction * duration;
  }

  function skip(seconds: number) {
    const video = videoRef.current;
    if (!video) return;
    video.currentTime = Math.min(duration, Math.max(0, video.currentTime + seconds));
  }

  const played = duration > 0 ? (position / duration) * 100 : 0;
  const bufferedPct = duration > 0 ? (buffered / duration) * 100 : 0;

  return (
    <div className="player">
      <video
        ref={videoRef}
        className="video"
        playsInline
        onLoadedMetadata={(e) => {
          setDuration(e.currentTarget.duration || 0);
          startedRef.current = true;
        }}
        onTimeUpdate={(e) => setPosition(e.currentTarget.currentTime)}
        onProgress={(e) => {
          const ranges = e.currentTarget.buffered;
          setBuffered(ranges.length ? ranges.end(ranges.length - 1) : 0);
        }}
        onPlay={() => setPaused(false)}
        onPause={() => setPaused(true)}
        onEnded={() => setPaused(true)}
        onError={() => setError("This item could not be played.")}
      >
        <source src={api.streamUrl(itemId)} />
        Your browser cannot play this file.
      </video>

      {error && <p className="error">{error}</p>}

      <div className="player-bar">
        <button type="button" className="primary compact" onClick={togglePlay}>
          {paused ? "▶" : "❚❚"}
        </button>

        <span className="time">{formatTime(position)}</span>

        <div
          className="scrub"
          onClick={seekTo}
          role="slider"
          aria-label="Seek"
          aria-valuemin={0}
          aria-valuemax={Math.floor(duration)}
          aria-valuenow={Math.floor(position)}
          tabIndex={0}
        >
          <div className="scrub-buffered" style={{ width: `${bufferedPct}%` }} />
          <div className="scrub-played" style={{ width: `${played}%` }} />
        </div>

        <span className="time">{formatTime(duration)}</span>

        <button type="button" className="link" onClick={() => skip(-30)} title="Back 30 seconds">
          ↺30
        </button>
        <button type="button" className="link" onClick={() => skip(30)} title="Forward 30 seconds">
          30↻
        </button>

        <input
          className="volume"
          type="range"
          min={0}
          max={1}
          step={0.05}
          value={muted ? 0 : volume}
          aria-label="Volume"
          onChange={(e) => {
            const next = Number(e.target.value);
            setVolume(next);
            setMuted(next === 0);
            if (videoRef.current) {
              videoRef.current.volume = next;
              videoRef.current.muted = false;
            }
          }}
        />

        <button type="button" className="link" onClick={onClose}>
          Close
        </button>
      </div>

      <p className="player-title">{title}</p>
    </div>
  );
}