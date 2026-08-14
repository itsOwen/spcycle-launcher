import { useEffect, useState } from "react";
import * as ipc from "@/lib/ipc";
import type { MediaSupport } from "@/lib/ipc";

const NONE: MediaSupport = { video: false, audioSink: false, h264: false };

export function useMediaSupport(): MediaSupport | null {
  const [support, setSupport] = useState<MediaSupport | null>(null);

  useEffect(() => {
    let live = true;
    ipc.mediaSupport().then(
      (s) => live && setSupport(s),
      () => live && setSupport(NONE),
    );
    return () => {
      live = false;
    };
  }, []);

  return support;
}

// what to install, in the user's words. null when nothing is missing.
export function missingFor(s: MediaSupport | null): string | null {
  if (!s || s.video) return null;
  if (!s.h264 && !s.audioSink) {
    return "This system has no H.264 decoder and no audio output for GStreamer, so the video backdrop cannot play. Installing gst-libav and gst-plugins-good enables it.";
  }
  if (!s.h264) {
    return "This system has no H.264 decoder for GStreamer, so the video backdrop cannot play. Installing gst-libav enables it.";
  }
  return "This system has no GStreamer audio output plugin, so the video backdrop cannot play. Installing gst-plugins-good enables it.";
}
