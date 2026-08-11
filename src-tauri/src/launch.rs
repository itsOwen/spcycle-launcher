// running windows binaries, natively on windows and through proton on linux.
//
// the server, the client loader and generate_ssl.exe all share one wine prefix,
// rooted at the game directory. that is what lets the in-prefix game trust the
// certificate the in-prefix server presents, since wine's certificate store is
// per-prefix. two prefixes would fail verification on every request.

use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tokio::process::Command;

#[cfg(unix)]
use crate::compat;
use crate::settings;

// spacewar: proton needs an app id to build a prefix for, and this is the one
// every non-steam title borrows
#[cfg(unix)]
pub const COMPAT_APP_ID: &str = "480";

// graphics settings and keybinds, carried across a prefix rebuild
#[cfg(unix)]
const GAME_SETTINGS_DIR: &str = "pfx/drive_c/users/steamuser/AppData/Local/Prospect";

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("Wine was not found. Install it, or choose a Proton build in Settings.")]
    NoWine,
    #[error("No Proton build was found. Install one through Steam.")]
    NoProton,
    #[error("The custom compatibility command in Settings does not point at a file.")]
    NoCustom,
    #[error("{0}")]
    Message(String),
}

// prefix_root is the directory whose compatdata holds the wine prefix. every
// caller must pass the same root, or certificate trust breaks. is_game selects
// proton's blocking verb and suppresses its console window.
pub fn wrap_exe(
    app: &AppHandle,
    exe: &Path,
    prefix_root: &Path,
    is_game: bool,
) -> Result<Command, LaunchError> {
    #[cfg(windows)]
    {
        let _ = (prefix_root, is_game, app);
        let mut cmd = Command::new(exe);
        cmd.current_dir(working_dir(exe, prefix_root));
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        Ok(cmd)
    }
    #[cfg(unix)]
    {
        wrap_unix(app, exe, prefix_root, is_game)
    }
}

// a bare filename's parent is Some(""), not None, and spawning with an empty
// working directory fails with ENOENT, so it must fall back to the prefix root
fn working_dir<'a>(exe: &'a Path, prefix_root: &'a Path) -> &'a Path {
    match exe.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => prefix_root,
    }
}

#[cfg(unix)]
fn wrap_unix(
    app: &AppHandle,
    exe: &Path,
    prefix_root: &Path,
    is_game: bool,
) -> Result<Command, LaunchError> {
    // waitforexitandrun settles the prefix first, which is what the game needs
    let verb = if is_game {
        "waitforexitandrun"
    } else {
        "runinprefix"
    };
    let info = compat::detect();
    let compatdata = prefix_root.join("compatdata");
    let _ = std::fs::create_dir_all(&compatdata);

    let mut cmd = match settings::compat_tool(app).as_str() {
        "proton" => {
            let proton = proton_exe(app, &info).ok_or(LaunchError::NoProton)?;
            let steam_root = info
                .steam_root
                .clone()
                .ok_or_else(|| LaunchError::Message("Steam was not found.".into()))?;

            prime_prefix(&proton, &steam_root, &compatdata, exe);

            let mut c = match compat::runtime_for(&proton, &info) {
                Some(run) => {
                    log::info!("running through the runtime this proton build asks for: {run}");
                    let mut c = Command::new(run);
                    c.arg("--").arg(&proton);
                    // the container must be able to write the install and prefix
                    let dir = prefix_root.display().to_string();
                    let existing =
                        std::env::var("PRESSURE_VESSEL_FILESYSTEMS_RW").unwrap_or_default();
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
            if is_game {
                // stops wine opening a console window behind the game
                let existing = std::env::var("WINEDLLOVERRIDES").unwrap_or_default();
                c.env(
                    "WINEDLLOVERRIDES",
                    if existing.is_empty() {
                        "conhost.exe=d".to_string()
                    } else {
                        format!("{existing};conhost.exe=d")
                    },
                );
            }
            c.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam_root)
                .env("STEAM_COMPAT_DATA_PATH", &compatdata)
                .env("STEAM_COMPAT_APP_ID", COMPAT_APP_ID)
                .env("SteamAppId", COMPAT_APP_ID)
                .env("SteamGameId", COMPAT_APP_ID);
            c
        }
        "custom" => {
            let raw = settings::compat_custom_cmd(app).ok_or(LaunchError::NoCustom)?;
            if !Path::new(&raw).is_file() {
                return Err(LaunchError::NoCustom);
            }
            let mut c = Command::new(&raw);
            c.arg(exe).env("WINEPREFIX", compatdata.join("pfx"));
            c
        }
        _ => {
            let wine = settings::wine_path(app).unwrap_or_else(|| "wine".into());
            if wine == "wine" && !info.wine {
                return Err(LaunchError::NoWine);
            }
            let mut c = Command::new(&wine);
            c.arg(exe)
                .env("WINEPREFIX", compatdata.join("pfx"))
                .env("WINEDEBUG", "-all");
            c
        }
    };

    scrub_appimage_env(&mut cmd);
    cmd.current_dir(working_dir(exe, prefix_root));
    Ok(cmd)
}

#[cfg(unix)]
fn proton_exe(app: &AppHandle, info: &compat::CompatInfo) -> Option<String> {
    if let Some(chosen) = settings::proton_path(app) {
        if Path::new(&chosen).is_file() {
            return Some(chosen);
        }
    }
    info.proton.first().cloned()
}

// build the prefix ahead of the real run, and rebuild it when proton changes
#[cfg(unix)]
fn prime_prefix(proton: &str, steam_root: &str, compatdata: &Path, exe: &Path) {
    let stamp_path = compatdata.join(".spc_proton");
    let stamp = std::fs::metadata(proton)
        .and_then(|m| m.modified())
        .map(|t| format!("{proton}\n{t:?}"))
        .unwrap_or_else(|_| proton.to_string());
    let previous = std::fs::read_to_string(&stamp_path).unwrap_or_default();
    if previous == stamp {
        return;
    }

    let same_build = previous.lines().next().is_some_and(|p| p == proton);
    let mut stash = None;
    if !previous.is_empty() && !same_build {
        log::info!("compatibility tool changed build, rebuilding the wine prefix");
        // graphics settings and keybinds live in the prefix; carry them over the wipe
        let saved = compatdata.with_extension("settings-stash");
        if !saved.exists() {
            let _ = std::fs::rename(compatdata.join(GAME_SETTINGS_DIR), &saved);
        }
        stash = saved.exists().then_some(saved);
        let _ = std::fs::remove_dir_all(compatdata);
        let _ = std::fs::create_dir_all(compatdata);
    }

    let mut cmd = std::process::Command::new(proton);
    cmd.arg("waitforexitandrun")
        .arg(exe)
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

    let built = compatdata.join("pfx").join("system.reg").is_file();
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
    // wineserver sits next to the wine binary
    let server = settings::wine_path(app)
        .map(PathBuf::from)
        .and_then(|w| w.parent().map(|p| p.join("wineserver")))
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

// variables an appimage injects into its own process, which proton and the game
// would otherwise inherit and break on
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
            // the url handler works whether steam is a package, a flatpak or a
            // windows install we could not locate in the registry
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

    // the prefix root must be the game directory for every binary, or the client
    // will not see the certificate the server presents
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

    // a bare filename's parent is Some("") and spawning with an empty working
    // directory fails with ENOENT. reg.exe is invoked bare, so this is the
    // difference between the wine certificate import working and failing.
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
