import { useCallback, useEffect, useState } from "react";
import { LazyStore } from "@tauri-apps/plugin-store";

// must match settings::STORE, both sides read and write the same file
const STORE = "storage.json";

// every key the backend reads, with the default it falls back to
const DEFAULTS = {
  game_directory: "" as string,
  proton_path: "" as string,
  autorun_steam: true as boolean,
  discord_presence: true as boolean,
  video_backdrop: true as boolean,
  video_sound: true as boolean,
  // percent, so a hand-edited storage.json cannot slip a float past the type guard
  video_volume: 60 as number,
  mongo_port: 0 as number,
  depot_blob_path: "" as string,
};

export type Settings = typeof DEFAULTS;
export type SettingKey = keyof Settings;

// autoSave off: every write saves explicitly, before the next launch reads it
const store = new LazyStore(STORE, { autoSave: false, defaults: {} });

export function useSettings() {
  const [settings, setSettings] = useState<Settings>(DEFAULTS);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const next = { ...DEFAULTS };
    for (const key of Object.keys(DEFAULTS) as SettingKey[]) {
      try {
        const value = await store.get(key);
        // a hand-edited storage.json must not put a string where a number belongs
        if (value !== null && value !== undefined && typeof value === typeof DEFAULTS[key]) {
          (next[key] as unknown) = value;
        }
      } catch {
        // a missing key is the default
      }
    }
    setSettings(next);
    setLoaded(true);
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const update = useCallback(
    async <K extends SettingKey>(key: K, value: Settings[K]) => {
      // optimistic: the control should not lag a disk write
      setSettings((prev) => ({ ...prev, [key]: value }));
      try {
        await store.set(key, value);
        await store.save();
        setError(null);
      } catch (e) {

        setError(`${key} could not be saved: ${String(e)}`);
        await refresh();
      }
    },
    [refresh],
  );

  return { settings, update, refresh, loaded, error };
}
