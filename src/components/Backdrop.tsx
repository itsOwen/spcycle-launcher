import { useEffect, useRef, useState } from "react";
import still from "@/assets/backdrop-static.png";
import * as ipc from "@/lib/ipc";
import type { MediaSupport } from "@/lib/ipc";

export function Backdrop({
  on,
  sound,
  volume,
  suspended,
  support,
}: {
  on: boolean;
  sound: boolean;
  volume: number;
  suspended: boolean;
  support: MediaSupport | null;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  const playbackStarted = useRef(false);
  const soundRef = useRef(sound);
  const [failed, setFailed] = useState(false);
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    ipc.mediaUrl().then(setSrc, () => setFailed(true));
  }, []);

  const canVideo = support?.video === true && !failed && !!src;
  const showVideo = on && canVideo;

  useEffect(() => {
    const el = ref.current;
    if (!showVideo || !el || suspended) return;
    playbackStarted.current = false;
    el.muted = true;
    el.play().then(() => {
      playbackStarted.current = true;
      el.muted = !soundRef.current;
    }, () => setFailed(true));
  }, [showVideo, suspended]);

  useEffect(() => {
    const el = ref.current;
    if (!el || !suspended) return;
    el.muted = true;
    el.pause();
  }, [suspended, showVideo]);

  useEffect(() => {
    // the play effect reads this ref, so it must be current before that runs
    soundRef.current = sound;
    const el = ref.current;
    if (el && playbackStarted.current && !suspended) el.muted = !sound;
  }, [sound, showVideo, suspended]);

  // its own effect: re-running play() on every slider step dropped the sound out
  useEffect(() => {
    const el = ref.current;
    if (el) el.volume = Math.min(1, Math.max(0, volume / 100));
  }, [volume, showVideo]);

  const replay = () => {
    const el = ref.current;
    if (!el || suspended) return;
    el.currentTime = 0;
    void el.play().catch(() => setFailed(true));
  };

  return (
    <div className="pointer-events-none fixed inset-0 -z-[1] overflow-hidden">
      <img src={still} alt="" aria-hidden className="h-full w-full object-cover" />
      {showVideo && (
        <video
          ref={ref}
          src={src}
          muted
          playsInline
          preload="auto"
          aria-hidden
          onEnded={replay}
          onError={() => setFailed(true)}
          className="absolute inset-0 h-full w-full object-cover"
        />
      )}
      <div className="absolute inset-0 bg-black/40" />
    </div>
  );
}
