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

// reads latest.json from the newest release and checks its signature
export function useUpdate(): UpdateState {
  const [available, setAvailable] = useState<Update | null>(null);
  const [label, setLabel] = useState("not checked");
  const [busy, setBusy] = useState(false);

  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

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

  useEffect(() => {
    const id = setTimeout(checkNow, 0);
    return () => clearTimeout(id);
  }, [checkNow]);

  return { available, label, busy, checkNow, install, dismiss };
}
