// windows binaries: native on windows, proton on linux. wine's certificate store
// is per-prefix, so the game and its cert trust share one; the server has its own.

use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tokio::process::Command;

#[cfg(unix)]
use crate::compat;
use crate::settings;

// spacewar: the app id every non-steam title borrows
#[cfg(unix)]
pub const COMPAT_APP_ID: &str = "480";

// graphics settings and keybinds, carried across a prefix rebuild
#[cfg(unix)]
const GAME_SETTINGS_DIR: &str = "pfx/drive_c/users/steamuser/AppData/Local/Prospect";

// windows has no proton to find and cannot fail here
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("No Proton build was found. Install one through Steam.")]
    NoProton,
    #[error("{0}")]
    Message(String),
}

// prefix_root holds the wine prefix. game and cert trust share one, server has its own.
// is_game picks the verb on unix. see wrap_unix.
pub fn wrap_exe(
    app: &AppHandle,
    exe: &Path,
    prefix_root: &Path,
    is_game: bool,
) -> Result<Command, LaunchError> {
    #[cfg(windows)]
    {
        let _ = (prefix_root, app, is_game);
        let mut cmd = Command::new(exe);
        cmd.current_dir(working_dir(exe, prefix_root));
        // tokio's Command has creation_flags inherently
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        Ok(cmd)
    }
    #[cfg(unix)]
    {
        wrap_unix(app, exe, prefix_root, is_game)
    }
}

// a bare filename's parent is Some(""), and spawning with that fails ENOENT
fn working_dir<'a>(exe: &'a Path, prefix_root: &'a Path) -> &'a Path {
    match exe.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => prefix_root,
    }
}

#[cfg(any(unix, test))]
fn verb_for(is_game: bool) -> &'static str {
    if is_game {
        "waitforexitandrun"
    } else {
        "runinprefix"
    }
}

#[cfg(unix)]
fn wrap_unix(
    app: &AppHandle,
    exe: &Path,
    prefix_root: &Path,
    is_game: bool,
) -> Result<Command, LaunchError> {
    // `run` puts the game behind the steam.exe shim, where it never starts, so it is
    // not an option. but runinprefix is not the alternative it looks like: proton
    // gates setup_prefix() on `argv[1] != "runinprefix"` and then dispatches it
    // straight to wine, skipping session setup and protonfixes entirely — half the
    // framerate and artifacts. waitforexitandrun reaches run() without the shim.
    let verb = verb_for(is_game);
    let info = compat::detect();
    let compatdata = prefix_root.join("compatdata");
    let _ = std::fs::create_dir_all(&compatdata);

    let mut cmd = {
        let proton = proton_exe(app, &info).ok_or(LaunchError::NoProton)?;
        let steam_root = info
            .steam_root
            .clone()
            .ok_or_else(|| LaunchError::Message("Steam was not found.".into()))?;

        prime_prefix(&proton, &steam_root, &compatdata);

        let mut c = match compat::runtime_for(&proton, &info) {
            Some(run) => {
                log::info!("running through the runtime this proton build asks for: {run}");
                let mut c = Command::new(run);
                c.arg("--").arg(&proton);
                // the container must be able to write the install and prefix
                let dir = prefix_root.display().to_string();
                let existing = std::env::var("PRESSURE_VESSEL_FILESYSTEMS_RW").unwrap_or_default();
                c.env(
                    "PRESSURE_VESSEL_FILESYSTEMS_RW",
                    if existing.is_empty() {
                        dir
                    } else {
                        format!("{existing}:{dir}")
                    },
                );
                c
            }
            None => Command::new(&proton),
        };
        c.arg(verb).arg(exe);
        // no conhost override: the loader passes -log and the game dies without a console
        c.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam_root)
            .env("STEAM_COMPAT_DATA_PATH", &compatdata)
            .env("STEAM_COMPAT_APP_ID", COMPAT_APP_ID)
            .env("SteamAppId", COMPAT_APP_ID)
            .env("SteamGameId", COMPAT_APP_ID);
        c
    };

    scrub_appimage_env(&mut cmd);
    cmd.current_dir(working_dir(exe, prefix_root));
    Ok(cmd)
}

#[cfg(unix)]
fn proton_exe(app: &AppHandle, info: &compat::CompatInfo) -> Option<String> {
    match settings::proton_path(app) {
        Some(chosen) if Path::new(&chosen).is_file() => Some(chosen),
        // falling back picks a different build than the user chose, and prime_prefix
        // reads that as "the tool changed" and wipes the prefix. loud on purpose.
        other => {
            let fallback = info.proton.first().cloned();
            log::warn!("no usable proton from settings ({other:?}); falling back to {fallback:?}");
            fallback
        }
    }
}

// build a prefix before anything reads or writes inside it. wrap_exe primes
// lazily, which is too late for the certificate: ensure_trusted asks the prefix
// whether the cert is already there, and a later prime that rebuilds the prefix
// throws that answer away — the game then starts with nothing to trust and fails
// sign-in with "Login Failed. Error code: 5", once, after every proton change.
#[cfg(unix)]
pub fn prepare_prefix(app: &AppHandle, prefix_root: &Path) {
    let info = compat::detect();
    let Some(proton) = proton_exe(app, &info) else {
        return;
    };
    let Some(steam_root) = info.steam_root.clone() else {
        return;
    };
    let compatdata = prefix_root.join("compatdata");
    let _ = std::fs::create_dir_all(&compatdata);
    prime_prefix(&proton, &steam_root, &compatdata);
}

// build the prefix ahead of the real run, and rebuild it when proton changes
#[cfg(unix)]
fn prime_prefix(proton: &str, steam_root: &str, compatdata: &Path) {
    let stamp_path = compatdata.join(".spc_proton");
    // canonicalised: ~/.steam/steam and ~/.local/share/Steam are one directory
    let id = std::fs::canonicalize(proton)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| proton.to_string());
    let stamp = std::fs::metadata(proton)
        .and_then(|m| m.modified())
        .map(|t| format!("{id}\n{t:?}"))
        .unwrap_or_else(|_| id.clone());
    let previous = std::fs::read_to_string(&stamp_path).unwrap_or_default();
    if previous == stamp {
        return;
    }

    let same_build = previous.lines().next().is_some_and(|p| p == id);
    let mut stash = None;
    if !previous.is_empty() && !same_build {
        log::warn!(
            "rebuilding {}: it was built with {}, and this launch resolved {id}",
            compatdata.display(),
            previous.lines().next().unwrap_or("(nothing)"),
        );
        // graphics settings and keybinds live in the prefix; carry them over the wipe
        let saved = compatdata.with_extension("settings-stash");
        if !saved.exists() {
            let _ = std::fs::rename(compatdata.join(GAME_SETTINGS_DIR), &saved);
        }
        stash = saved.exists().then_some(saved);
        let _ = std::fs::remove_dir_all(compatdata);
        let _ = std::fs::create_dir_all(compatdata);
    }

    // `cmd /c exit`, never the real exe, which would start a second copy of it.
    // `run` and not runinprefix: only `run` reaches setup_prefix(), which creates it.
    let mut cmd = std::process::Command::new(proton);
    cmd.arg("run")
        .arg("cmd")
        .arg("/c")
        .arg("exit")
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_root)
        .env("STEAM_COMPAT_DATA_PATH", compatdata)
        .env("STEAM_COMPAT_APP_ID", COMPAT_APP_ID)
        .env("SteamAppId", COMPAT_APP_ID)
        .env("SteamGameId", COMPAT_APP_ID)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for var in APPIMAGE_ENV {
        cmd.env_remove(var);
    }
    log::info!("preparing the wine prefix");
    let _ = cmd.status();

    // tracked_files, not system.reg: only setup_prefix() writes it
    let built = compatdata.join("tracked_files").is_file();
    if built {
        let _ = std::fs::write(&stamp_path, stamp);
    } else {
        log::warn!("the wine prefix was not built; will retry next launch");
    }
    if let Some(saved) = stash {
        let dest = compatdata.join(GAME_SETTINGS_DIR);
        let restored = built
            && dest
                .parent()
                .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
            && std::fs::rename(&saved, &dest).is_ok();
        if !restored {
            log::warn!(
                "game settings kept at {} until the prefix rebuilds",
                saved.display()
            );
        }
    }
}

// shut the prefix down after the game exits, so a later run starts clean
#[cfg(unix)]
pub fn reset_prefix(app: &AppHandle, prefix_root: &Path) {
    let prefix = prefix_root.join("compatdata").join("pfx");
    if !prefix.join("system.reg").is_file() {
        return;
    }
    // proton's own wineserver; the one on PATH would not know this prefix
    let info = compat::detect();
    let server = proton_exe(app, &info)
        .map(PathBuf::from)
        .and_then(|p| p.parent().map(|d| d.join("files/bin/wineserver")))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("wineserver"));

    let mut cmd = std::process::Command::new(server);
    cmd.arg("-k")
        .env("WINEPREFIX", &prefix)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for var in APPIMAGE_ENV {
        cmd.env_remove(var);
    }
    let _ = cmd.status();
}

// vars an appimage injects that proton and the game would break on
#[cfg(unix)]
pub(crate) const APPIMAGE_ENV: &[&str] = &[
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONDONTWRITEBYTECODE",
    "ARGV0",
    "OWD",
    "GDK_BACKEND",
    "GDK_PIXBUF_MODULE_FILE",
    "GTK_THEME",
    "GTK_PATH",
    "GTK_DATA_PREFIX",
    "GTK_EXE_PREFIX",
    "GTK_IM_MODULE_FILE",
    "WEBKIT_DISABLE_DMABUF_RENDERER",
    "XDG_DATA_DIRS",
    "GIO_EXTRA_MODULES",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GST_PLUGIN_PATH",
    "GST_PLUGIN_PATH_1_0",
    "GST_PLUGIN_SCANNER",
    "GST_PLUGIN_SCANNER_1_0",
    "GST_PTP_HELPER_1_0",
    "GST_REGISTRY_REUSE_PLUGIN_SCANNER",
    "GSETTINGS_SCHEMA_DIR",
    "WEBKIT_EXEC_PATH",
    "APPDIR",
];

#[cfg(unix)]
pub(crate) fn scrub_appimage_env(cmd: &mut Command) {
    for var in APPIMAGE_ENV {
        cmd.env_remove(var);
    }
}

// ---- steam ----

// steam's own executable, if we can find it
#[cfg(windows)]
fn steam_exe() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Valve\\Steam")
        .ok()?;
    let path: String = key.get_value("SteamPath").ok()?;
    let exe = PathBuf::from(path.replace('/', "\\")).join("steam.exe");
    exe.is_file().then_some(exe)
}

#[cfg(unix)]
fn steam_exe() -> Option<PathBuf> {
    None
}

// the game authenticates through the steam client, so this is not optional
pub async fn maybe_start_steam(app: &AppHandle) {
    if crate::proc::steam_is_running() {
        return;
    }
    if !settings::autorun_steam(app) {
        return;
    }

    log::info!("starting Steam");
    match steam_exe() {
        Some(exe) => {
            let _ = std::process::Command::new(exe)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        None => {
            // works whether steam is a package, a flatpak, or unlocatable
            let _ = open::that_detached("steam://open/main");
        }
    }

    // steam takes a few seconds to register its process
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if crate::proc::steam_is_running() {
            log::info!("Steam is up");
            return;
        }
    }
    log::warn!("Steam did not start within 10s");
}

#[cfg(test)]
mod tests {
    use super::*;

    // proton gates setup_prefix() on `argv[1] != "runinprefix"` and dispatches that
    // verb straight to wine, so it never reaches run(): no session setup, no
    // protonfixes. the game ran at half the framerate with artifacts until this
    // was a waitforexitandrun. only `run` goes behind the steam.exe shim.
    #[test]
    fn the_game_never_launches_with_the_verb_that_skips_proton_setup() {
        assert_eq!(verb_for(true), "waitforexitandrun");
        assert_ne!(verb_for(true), "runinprefix");
        assert_ne!(verb_for(true), "run");
        // helpers want the cheap verb: they need the prefix, not a proton session
        assert_eq!(verb_for(false), "runinprefix");
    }

    // the game's prefix, or the client will not see the server's certificate
    #[test]
    fn the_appimage_scrub_list_covers_the_loader_and_python_vars() {
        #[cfg(unix)]
        {
            for must in ["LD_LIBRARY_PATH", "LD_PRELOAD", "PYTHONHOME", "APPDIR"] {
                assert!(
                    APPIMAGE_ENV.contains(&must),
                    "{must} must be scrubbed or proton inherits the appimage's runtime"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_compat_app_id_is_spacewar() {
        assert_eq!(COMPAT_APP_ID, "480");
    }

    // reg.exe is invoked bare, and an empty working directory spawns ENOENT
    #[test]
    fn a_bare_filename_runs_from_the_prefix_root() {
        let root = Path::new("/games/spcycle");

        assert_eq!(working_dir(Path::new("reg.exe"), root), root);
        assert_eq!(working_dir(Path::new(""), root), root);

        // a real path still runs from its own directory
        assert_eq!(
            working_dir(
                Path::new("/games/spcycle/Prospect/Binaries/Win64/x.exe"),
                root
            ),
            Path::new("/games/spcycle/Prospect/Binaries/Win64")
        );
    }

    // proves the failure mode the fix prevents, so nobody simplifies it back
    #[test]
    fn spawning_with_an_empty_working_directory_fails() {
        let empty = Path::new("reg.exe").parent().unwrap();
        assert!(empty.as_os_str().is_empty());

        let err = std::process::Command::new(if cfg!(windows) { "cmd" } else { "/bin/echo" })
            .current_dir(empty)
            .output();
        assert!(
            err.is_err(),
            "an empty working directory must fail, or this guard is unnecessary"
        );
    }
}
