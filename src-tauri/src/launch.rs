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

// nothing constructs these on windows: there is no proton to find, and wrap_exe's
// windows branch cannot fail
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("No Proton build was found. Install one through Steam.")]
    NoProton,
    #[error("{0}")]
    Message(String),
}

// prefix_root is the directory whose compatdata holds the wine prefix. the game
// and the certificate trust must pass the same root, or the game does not trust
// the server; the server passes its own. see game::play.
pub fn wrap_exe(app: &AppHandle, exe: &Path, prefix_root: &Path) -> Result<Command, LaunchError> {
    #[cfg(windows)]
    {
        let _ = (prefix_root, app);
        let mut cmd = Command::new(exe);
        cmd.current_dir(working_dir(exe, prefix_root));
        // tokio's Command has creation_flags inherently; the CommandExt trait
        // std::process::Command needs is not in play here
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        Ok(cmd)
    }
    #[cfg(unix)]
    {
        wrap_unix(app, exe, prefix_root)
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
fn wrap_unix(app: &AppHandle, exe: &Path, prefix_root: &Path) -> Result<Command, LaunchError> {
    // always runinprefix, for the game as much as anything else.
    //
    // never waitforexitandrun: it runs `wineserver -w` first, which blocks until
    // every wine process in the prefix has exited, and we deliberately keep the
    // server running in this same prefix for the whole session. the game would
    // wait for a server that never exits.
    //
    // not `run` either, though it is tempting: it is the verb steam itself uses,
    // and unlike runinprefix it calls setup_prefix(). but it launches the target
    // through c:\windows\system32\steam.exe, and under that shim the game loads,
    // lets ue4ss hook, then exits after ~12 s without ever bringing up the engine
    // — no Prospect.log is written at all. measured, not theorised.
    //
    // the steam bridge buys us nothing anyway: the client patch points the game at
    // the local server, so no steam ticket is ever requested. prime_prefix uses
    // `run` for the setup_prefix() half, which is the only part we actually want.
    //
    // runinprefix was once blamed for the loader failing to patch the game
    // ("Failed to write memory. Error: 5") and it was never the cause. that write
    // fails when something else is already living in the game's prefix, whatever
    // verb is in play, which is why the server now gets a prefix of its own. under
    // runinprefix with the prefix to itself the loader patches every site — and
    // under `run` the game dies regardless. measured, both ways.
    let verb = "runinprefix";
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
        // conhost is deliberately left alone. disabling it (`conhost.exe=d`) hides
        // the console window that appears behind the game, but the loader starts
        // the game with -log, and a game that asks for a console it cannot get
        // dies during startup without writing so much as a Prospect.log. measured
        // both ways: the window is the price of the game starting at all.
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
    if let Some(chosen) = settings::proton_path(app) {
        if Path::new(&chosen).is_file() {
            return Some(chosen);
        }
    }
    info.proton.first().cloned()
}

// build the prefix ahead of the real run, and rebuild it when proton changes
#[cfg(unix)]
fn prime_prefix(proton: &str, steam_root: &str, compatdata: &Path) {
    let stamp_path = compatdata.join(".spc_proton");
    // canonicalised. ~/.steam/steam and ~/.local/share/Steam are one directory
    // reached by two names, and comparing the raw strings made the same proton
    // build read as a different one. the answer to a different build is to delete
    // the prefix, so this compared paths into a loop that wiped compatdata on
    // every launch and never rebuilt it.
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

    // `cmd /c exit`, never the exe we are about to launch. proton builds the
    // prefix before running whatever it is given, so anything trivial primes it —
    // and priming with the real exe starts a second copy of it. with the server
    // that copy runs from the launcher's own directory, finds no appsettings.json,
    // binds kestrel's default port, and never exits, which then deadlocks every
    // later `waitforexitandrun` in this prefix.
    //
    // `run`, not `runinprefix`. proton dispatches
    // `init_session(sys.argv[1] != "runinprefix")`, so only `run` reaches
    // setup_prefix(), and that is the only thing that can *create* a prefix.
    // priming with runinprefix after a wipe leaves it wiped: "the wine prefix was
    // not built" on every launch, and the next launch wipes it again.
    //
    // priming only. the game itself is still launched with runinprefix.
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

    // tracked_files, not system.reg: plain wine bootstraps a prefix on first use,
    // so system.reg appears whether or not proton ever set one up. only
    // setup_prefix() writes tracked_files, and priming uses `run`, so it does.
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
    // proton ships its own wineserver; the one on PATH, if any, belongs to a
    // different wine build and would not know this prefix
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
