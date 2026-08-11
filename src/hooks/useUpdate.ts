import { useCallback, useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export interface UpdateState {
  available: Update | null;
  label: string;
  busy: boolean;
  checkNow: () => void;
  install: () => void;
  dismiss: () => void;
}

// the updater reads latest.json from the newest github release and verifies the
// download against the signature recorded there. tools/release.sh writes it.
export function useUpdate(): UpdateState {
  const [available, setAvailable] = useState<Update | null>(null);
  const [label, setLabel] = useState("not checked");
  const [busy, setBusy] = useState(false);

  const live = useRef(true);
  useEffect(
    () => () => {
      live.current = false;
    },
    [],
  );

  const checkNow = useCallback(() => {
    setBusy(true);
    setLabel("checking…");
    check().then(
      (found) => {
        if (!live.current) return;
        setAvailable(found ?? null);
        setLabel(found ? `${found.version} is available` : "up to date");
        setBusy(false);
      },
      (e: unknown) => {
        if (!live.current) return;
        setLabel(String(e));
        setBusy(false);
      },
    );
  }, []);

  const install = useCallback(() => {
    if (!available) return;
    setBusy(true);
    setLabel("downloading…");
    // the installer takes over from here; on linux the AppImage is replaced
    available
      .downloadAndInstall()
      .then(() => relaunch())
      .catch((e: unknown) => {
        if (!live.current) return;
        setLabel(String(e));
        setBusy(false);
      });
  }, [available]);

  const dismiss = useCallback(() => setAvailable(null), []);

  // one check on boot. an update nobody is told about is not an update, and the
  // alternative was a button on a tab most people never open. a failure here
  // stays quiet: a launcher that cannot reach github still runs the game.
  useEffect(checkNow, [checkNow]);

  return { available, label, busy, checkNow, install, dismiss };
}
