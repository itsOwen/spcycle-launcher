// the only file importing @tauri-apps/*, so the surface is one file wide
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Phase =
  | "NEEDS_COMPONENTS"
  | "NEEDS_GAME"
  | "INSTALLING_COMPONENTS"
  | "DOWNLOADING"
  | "PAUSED"
  | "VERIFYING"
  | "READY"
  | "STARTING"
  | "PLAYING"
  | "UNINSTALLING"
  | "UPDATING";

export type ServiceState = "down" | "starting" | "up" | "failed";

export interface Services {
  mongo: ServiceState;
  server: ServiceState;
  steam: ServiceState;
}

export interface InstallState {
  gameFiles: boolean;
  components: boolean;
  manifestId: string;
  partial: boolean;
}

export interface DepotInfo {
  depotId: number;
  // u64, so it arrives as a string: it exceeds Number.MAX_SAFE_INTEGER
  manifestId: string;
  files: number;
  // what lands on disk
  totalBytes: number;
  // what actually crosses the network, 2.4x smaller than totalBytes
  compressedBytes: number;
  createdAt: number;
}

export interface Snapshot {
  phase: Phase;
  install: InstallState;
  services: Services;
  launcherVersion: string;
  componentsVersion: string | null;
  gameBytes: number;
  freeBytes: number;
  gameDirectory: string;
}

export interface CompatInfo {
  supported: boolean;
  // absolute paths to every proton launcher found
  proton: string[];
  // steam runtime app id -> entry point
  runtimes: Record<string, string>;
  steamRoot: string | null;
}

export type Severity = "blocking" | "degraded";

export interface Check {
  id: string;
  title: string;
  impact: string;
  ok: boolean;
  severity: Severity;
  install: string | null;
}

export interface Preflight {
  distro: string;
  checks: Check[];
  allOk: boolean;
  hasBlocking: boolean;
}

export interface UninstallItem {
  label: string;
  path: string;
  bytes: number;
  removable: boolean;
  note: string | null;
}

export type LogKind = "launcher" | "mongod" | "server" | "loader" | "game";

// ---- commands ----

export const launcherState = () => invoke<Snapshot>("launcher_state");
export const preflight = () => invoke<Preflight>("preflight");
export const detectCompatTools = () => invoke<CompatInfo>("detect_compat_tools");
export const depotInfo = () => invoke<DepotInfo>("depot_info");

export const installComponents = () => invoke<void>("install_components");
export const installGame = () => invoke<void>("install_game");
export const pauseDownload = () => invoke<void>("pause_download");
export const verifyAndRepair = () => invoke<void>("verify_and_repair");

export const play = () => invoke<number>("play");
export const stopGame = () => invoke<void>("stop_game");

export const uninstallPlan = () => invoke<UninstallItem[]>("uninstall_plan");
export const uninstallEverything = () => invoke<void>("uninstall_everything");

export const pickGameDirectory = () => invoke<string | null>("pick_game_directory");
export const openGameFolder = () => invoke<void>("open_game_folder");
export const openLauncherFolder = () => invoke<void>("open_launcher_folder");
export const logTail = (which: LogKind, lines = 200) =>
  invoke<string>("log_tail", { which, lines });

// ---- events ----

export const onPhase = (cb: (p: Phase) => void): Promise<UnlistenFn> =>
  listen<Phase>("phase", (e) => cb(e.payload));

export const onServices = (cb: (s: Services) => void): Promise<UnlistenFn> =>
  listen<Services>("services", (e) => cb(e.payload));

export const onProgress = (
  cb: (done: number, total: number, label: string) => void,
): Promise<UnlistenFn> =>
  listen<[number, number, string]>("progress", (e) => cb(...e.payload));

export const onProgressBar = (cb: (show: boolean) => void): Promise<UnlistenFn> =>
  listen<boolean>("progressBar", (e) => cb(e.payload));

export const onProgressPausable = (cb: (can: boolean) => void): Promise<UnlistenFn> =>
  listen<boolean>("progressPausable", (e) => cb(e.payload));

export const onNotify = (
  cb: (text: string, level: 0 | 1 | 2) => void,
): Promise<UnlistenFn> =>
  listen<[string, 0 | 1 | 2]>("notify", (e) => cb(...e.payload));
