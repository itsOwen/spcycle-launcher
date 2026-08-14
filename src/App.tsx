import { useCallback, useEffect, useRef, useState } from "react";
import * as ipc from "@/lib/ipc";
import type { Phase, Services, Snapshot } from "@/lib/ipc";
import { bytes } from "@/lib/format";
import { Titlebar } from "./components/Titlebar";
import { Backdrop } from "./components/Backdrop";
import { BackdropControls } from "./components/BackdropControls";
import { useSettings } from "./hooks/useSettings";
import { AboutTab } from "./components/AboutTab";
import { Rail, type Tab } from "./components/Rail";
import { LaunchButton } from "./components/LaunchButton";
import { ProgressBar, type Progress } from "./components/ProgressBar";
import { Toasts, type Toast } from "./components/Toasts";
import { PlayTab } from "./components/PlayTab";
import { StashTab } from "./components/StashTab";
import { FilesTab } from "./components/FilesTab";
import { ServerTab } from "./components/ServerTab";
import { ConfigTab } from "./components/ConfigTab";
import { UninstallDialog } from "./components/UninstallDialog";
import { PreflightDialog } from "./components/PreflightDialog";
import { BootSplash } from "./components/BootSplash";
import { UpdateBanner } from "./components/UpdateBanner";
import { useUpdate } from "@/hooks/useUpdate";
import { useMediaSupport, missingFor } from "@/hooks/useMediaSupport";

const DOWN: Services = { mongo: "down", server: "down", steam: "down" };

const IDLE_POLL_MS = 3000;
// while events are driving the ui, a slow poll is only a safety net
const BUSY_POLL_MS = 15000;

const BOOT: Snapshot = {
  phase: "NEEDS_COMPONENTS",
  install: { gameFiles: false, components: false, manifestId: "0", partial: false },
  services: DOWN,
  launcherVersion: "",
  componentsVersion: null,
  gameBytes: 0,
  freeBytes: 0,
  gameDirectory: "",
};

const ZERO: Progress = {
  done: 0,
  total: 0,
  label: "",
  bytesPerSecond: 0,
  secondsLeft: null,
};

export default function App() {
  // `update` here is the launcher's own updater, so the settings writer is renamed
  const { settings, update: setSetting, loaded: settingsLoaded } = useSettings();
  const [tab, setTab] = useState<Tab>("play");
  const [snap, setSnap] = useState<Snapshot>(BOOT);
  const [progress, setProgress] = useState<Progress>(ZERO);
  const [showBar, setShowBar] = useState(false);
  const [pausable, setPausable] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [playingSince, setPlayingSince] = useState<number | null>(null);
  const [uninstalling, setUninstalling] = useState(false);
  const [checks, setChecks] = useState<ipc.Preflight | null>(null);
  const [booting, setBooting] = useState(true);
  const [ready, setReady] = useState(false);
  const update = useUpdate();
  const mediaSupport = useMediaSupport();
  const mediaBlocked = missingFor(mediaSupport);

  // exponential smoothing: raw byte rates are far too jittery to show
  const rateRef = useRef({ at: 0, done: 0, smoothed: 0 });

  const endBoot = useCallback(() => setBooting(false), []);

  // replies can land out of order, and a stale snapshot would revert the phase
  const refreshSeq = useRef(0);

  const refresh = useCallback(() => {
    const seq = ++refreshSeq.current;
    ipc.launcherState().then(
      (next) => {
        if (seq === refreshSeq.current) setSnap(next);
        setReady(true);
      },
      // the splash must not hang on a backend that never answers
      () => setReady(true),
    );
  }, []);

  const toast = useCallback((text: string, level: 0 | 1 | 2) => {
    setToasts((prev) => {
      // consecutive duplicates are noise, not information
      if (prev.length > 0 && prev[prev.length - 1].text === text) return prev;
      const next = [...prev, { id: Date.now() + Math.random(), text, level }];
      return next.slice(-5);
    });
  }, []);

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  // auto-dismiss info and success; errors stay until clicked
  useEffect(() => {
    const soft = toasts.find((t) => t.level < 2);
    if (!soft) return;
    const id = setTimeout(() => dismiss(soft.id), 15000);
    return () => clearTimeout(id);
  }, [toasts, dismiss]);

  useEffect(() => {
    const unlisten: Promise<() => void>[] = [
      ipc.onPhase((phase: Phase) => {
        setSnap((s) => ({ ...s, phase }));
        setPlayingSince((was) =>
          phase === "PLAYING" ? (was ?? Date.now()) : null,
        );
        // a phase change means the rest of the snapshot may be stale too
        refresh();
      }),

      ipc.onServices((services) => setSnap((s) => ({ ...s, services }))),

      ipc.onProgress((done, total, label) => {
        const now = performance.now();
        const prev = rateRef.current;
        let smoothed = prev.smoothed;
        if (prev.at > 0 && now > prev.at && done >= prev.done) {
          const instant = ((done - prev.done) * 1000) / (now - prev.at);
          smoothed = prev.smoothed === 0 ? instant : prev.smoothed * 0.8 + instant * 0.2;
        }
        rateRef.current = { at: now, done, smoothed };
        setProgress({
          done,
          total,
          label,
          bytesPerSecond: smoothed,
          secondsLeft:
            total > done && smoothed > 1 ? (total - done) / smoothed : null,
        });
      }),

      ipc.onProgressBar((show) => {
        setShowBar(show);
        if (!show) {
          rateRef.current = { at: 0, done: 0, smoothed: 0 };
          setProgress(ZERO);
        }
      }),

      ipc.onProgressPausable(setPausable),

      ipc.onNotify(toast),
    ];

    refresh();
    return () => {
      unlisten.forEach((p) => p.then((f) => f()).catch(() => {}));
    };
  }, [refresh, toast]);

  // one environment check on boot; only shown if something is actually wrong
  useEffect(() => {
    ipc.preflight().then(
      (report) => {
        if (!report.allOk) setChecks(report);
      },
      () => {},
    );
  }, []);

  useEffect(() => {
    const idle = !showBar && snap.phase !== "PLAYING" && snap.phase !== "STARTING";
    const id = setInterval(refresh, idle ? IDLE_POLL_MS : BUSY_POLL_MS);
    return () => clearInterval(id);
  }, [showBar, snap.phase, refresh]);

  const onAction = useCallback(
    (action: "install" | "pause" | "play" | "stop") => {
      const run = {
        install: ipc.installGame,
        pause: ipc.pauseDownload,
        play: ipc.play,
        stop: ipc.stopGame,
      }[action];
      run().catch((e: unknown) => {
        // pausing is normal, and a level-2 toast never auto-dismisses
        if (String(e) === "Paused.") return;
        toast(String(e), 2);
      });
    },
    [toast],
  );

  const sub =
    snap.phase === "NEEDS_GAME"
      ? bytes(snap.freeBytes) + " free"
      : snap.phase === "PAUSED"
        ? bytes(snap.gameBytes) + " on disk"
        : undefined;

  return (
    <div className="flex h-full flex-col">
      {booting && <BootSplash ready={ready} onDone={endBoot} />}

      {/* held until the splash is gone, or its audio plays over the boot animation */}
      <Backdrop
        on={!booting && settingsLoaded && settings.video_backdrop}
        sound={settings.video_sound}
        volume={settings.video_volume}
        suspended={snap.phase === "STARTING" || snap.phase === "PLAYING"}
        support={mediaSupport}
      />

      <Titlebar version={snap.launcherVersion} />

      <div className="flex min-h-0 flex-1">
        <Rail tab={tab} onTab={setTab} services={snap.services} />

        <main className="relative flex min-w-0 flex-1 flex-col p-6">
          {!booting && <UpdateBanner update={update} />}

          {/* keyed on the tab, so switching replays the entrance */}
          <div key={tab} className="rise min-h-0 flex-1 overflow-auto">
            {tab === "play" && <PlayTab snap={snap} since={playingSince} />}
            {tab === "stash" && (
              <StashTab
                snap={snap}
                onError={(m) => toast(m, 2)}
                onNotify={toast}
              />
            )}
            {tab === "files" && (
              <FilesTab
                snap={snap}
                onUninstall={() => setUninstalling(true)}
                onChanged={refresh}
                onError={(m) => toast(m, 2)}
              />
            )}
            {tab === "server" && <ServerTab services={snap.services} />}
            {tab === "config" && (
              <ConfigTab version={snap.launcherVersion} update={update} />
            )}
            {tab === "about" && <AboutTab version={snap.launcherVersion} />}
          </div>

          <div className="rise mt-6 flex shrink-0 items-end gap-4" style={{ animationDelay: "120ms" }}>
            <BackdropControls
              video={settings.video_backdrop}
              sound={settings.video_sound}
              volume={settings.video_volume}
              // a click before the store has been read would be overwritten by it
              ready={settingsLoaded}
              blocked={mediaBlocked}
              onVideo={(v) => setSetting("video_backdrop", v)}
              onSound={(v) => setSetting("video_sound", v)}
              onVolume={(v) => setSetting("video_volume", v)}
              onBlocked={(reason) => toast(reason, 2)}
            />

            <div className="min-w-0 flex-1">
              {showBar && (
                <ProgressBar
                  progress={progress}
                  pausable={pausable}
                  onPause={() => onAction("pause")}
                />
              )}
            </div>
            <LaunchButton phase={snap.phase} sub={sub} onAction={onAction} />
          </div>

          <Toasts toasts={toasts} onDismiss={dismiss} />

          {uninstalling && (
            <UninstallDialog
              onClose={() => setUninstalling(false)}
              onDone={() => {
                setUninstalling(false);
                refresh();
              }}
            />
          )}

          {checks && !booting && (
            <PreflightDialog report={checks} onDismiss={() => setChecks(null)} />
          )}
        </main>
      </div>
    </div>
  );
}
