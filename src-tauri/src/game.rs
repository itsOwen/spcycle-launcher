// install state on disk, and the depot passes that change it.
// no database of what is installed: every question is answered from the filesystem.

use std::path::Path;

use tauri::AppHandle;
use tokio::process::Child;

use crate::state::ServiceState;
use crate::{cert, depot, launch, mongo, proc, settings, show_bar, show_pause};

// written after a completed pass, holding the manifest id it verified
pub const DEPOT_MARKER: &str = ".spc_depot";

// present only while a pass is unfinished, which is what makes the ui offer resume
pub const DEPOT_PARTIAL: &str = ".spc_partial";

#[derive(Debug, thiserror::Error)]
pub enum GameError {
    #[error("Not enough space: {needed} is needed and only {available} is free.")]
    NoSpace { needed: String, available: String },
    #[error("The download stopped: {0}")]
    Interrupted(String),
    #[error("Paused.")]
    Paused,
    #[error("{0}")]
    Message(String),
}

pub fn installed_manifest(dir: &Path) -> Option<u64> {
    std::fs::read_to_string(dir.join(DEPOT_MARKER))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn depot_ok(dir: &Path, manifest_id: u64) -> bool {
    manifest_id != 0 && installed_manifest(dir) == Some(manifest_id)
}

pub fn has_partial(dir: &Path) -> bool {
    dir.join(DEPOT_PARTIAL).is_file()
}

// walks the tree, so callers must run it off the ui thread
pub fn size_on_disk(dir: &Path) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|e| match e.file_type() {
                // never follow symlinks: a link into $HOME is not part of the install
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    walk(dir)
}

// install, update, resume and repair are all this function: the pass runs with
// verify(true), so what is already correct on disk is skipped
async fn depot_pass(app: &AppHandle, label: &str) -> Result<(), GameError> {
    let dir = settings::game_directory(app);

    std::fs::create_dir_all(&dir)
        .map_err(|e| GameError::Message(format!("Could not create {}: {e}", dir.display())))?;

    // breadcrumb first, so a hard kill mid-pass still leaves the evidence
    let _ = std::fs::write(dir.join(DEPOT_PARTIAL), b"");

    show_bar(app, true);
    show_pause(app, true);
    let result = depot::run(app, &dir, label).await;
    show_pause(app, false);
    show_bar(app, false);

    match result {
        Ok(manifest_id) => {
            std::fs::write(dir.join(DEPOT_MARKER), manifest_id.to_string()).map_err(|e| {
                GameError::Message(format!("Could not record the completed install: {e}"))
            })?;
            let _ = std::fs::remove_file(dir.join(DEPOT_PARTIAL));
            Ok(())
        }
        // a paused pass keeps its breadcrumb: that is what makes it resumable
        Err(e) => Err(e),
    }
}

pub async fn download_game(app: &AppHandle) -> Result<(), GameError> {
    depot_pass(app, "Downloading game files").await
}

pub async fn verify_and_repair(app: &AppHandle) -> Result<(), GameError> {
    depot_pass(app, "Verifying game files").await
}

// ---- launching ----

pub const LOADER_EXE: &str = "Prospect.Client.Loader.exe";
pub const SERVER_EXE: &str = "Prospect.Server.Api.exe";
// the loader reads this to tell steamworks which app it is. its absence is the
// documented cause of "Login Failed. Error code: 3".
const STEAM_APPID_TXT: &str = "steam_appid.txt";

// how long the server gets to start serving
const SERVER_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
// how long the game gets to appear after the loader starts
const GAME_APPEAR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

// shuts everything down in reverse start order, however play ends.
// deliberately no disarm(): success and rollback cannot drift apart.
struct Teardown<'a> {
    app: &'a AppHandle,
    prefix_root: std::path::PathBuf,
    loader: Option<Child>,
    server: Option<Child>,
    mongo: Option<mongo::Mongo>,
}

impl Drop for Teardown<'_> {
    fn drop(&mut self) {
        if let Some(mut c) = self.loader.take() {
            let _ = c.start_kill();
        }
        if let Some(mut c) = self.server.take() {
            let _ = c.start_kill();
        }
        // anything still running out of the game directory, including the game
        let killed = proc::kill_under(&self.prefix_root);
        if killed > 0 {
            log::info!("stopped {killed} game process(es)");
        }
        #[cfg(unix)]
        launch::reset_prefix(self.app, &self.prefix_root);

        if let Some(mut m) = self.mongo.take() {
            m.stop();
        }
        crate::presence::clear();
        crate::set_service(self.app, |s| {
            s.mongo = ServiceState::Down;
            s.server = ServiceState::Down;
            s.steam = ServiceState::Down;
        });
    }
}

fn log_file(app: &AppHandle, name: &str) -> std::fs::File {
    let path = settings::log_path(app, name);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap_or_else(|_| std::fs::File::create(&path).expect("a writable data directory"))
}

pub async fn play(app: &AppHandle) -> Result<i32, GameError> {
    let dir = settings::game_directory(app);
    let server_dir = settings::server_dir(app);

    // everything shares one prefix, rooted at the game directory. see launch.rs.
    let prefix_root = dir.clone();

    let mut down = Teardown {
        app,
        prefix_root: prefix_root.clone(),
        loader: None,
        server: None,
        mongo: None,
    };

    // 0 - gate. files can vanish between the phase poll and the button press.
    if !crate::components::complete(app) {
        return Err(GameError::Message(
            "The launcher components are missing. Reinstall them from the Files tab.".into(),
        ));
    }
    let manifest_id = depot::describe(app)?.manifest_id.parse().unwrap_or(0);
    if !depot_ok(&dir, manifest_id) {
        return Err(GameError::Message(
            "The game files are incomplete. Run Verify & repair.".into(),
        ));
    }

    // 1 — mongod.
    crate::set_service(app, |s| s.mongo = ServiceState::Starting);
    let mongo = match mongo::start(app).await {
        Ok(m) => m,
        Err(e) => {
            crate::set_service(app, |s| s.mongo = ServiceState::Failed);
            return Err(GameError::Message(e.to_string()));
        }
    };
    let mongo_uri = mongo.uri();
    down.mongo = Some(mongo);
    crate::set_service(app, |s| s.mongo = ServiceState::Up);

    // 2 - certificate. generated once ever, trusted once per prefix.
    let leaf = cert::ensure_cert(app, &prefix_root)
        .await
        .map_err(|e| GameError::Message(e.to_string()))?;
    cert::ensure_trusted(app, &leaf, &prefix_root)
        .await
        .map_err(|e| GameError::Message(e.to_string()))?;

    // 3 - steam. the game authenticates through it.
    launch::maybe_start_steam(app).await;
    if proc::steam_is_running() {
        crate::set_service(app, |s| s.steam = ServiceState::Up);
    } else {
        crate::set_service(app, |s| s.steam = ServiceState::Failed);
        // no recovery on linux: proton's steam bridge supplies the ticket, and
        // without it the loader fails with error code 3
        #[cfg(unix)]
        return Err(GameError::Message(
            "Steam is not running. Start Steam and sign in, then try again.".into(),
        ));
        #[cfg(windows)]
        crate::notify(
            app,
            "Steam does not appear to be running. If the game fails to sign in, start Steam and try again.",
            crate::NOTIFY_INFO,
        );
    }

    // 4 — the local server.
    crate::set_service(app, |s| s.server = ServiceState::Starting);
    let server_exe = server_dir.join(SERVER_EXE);
    if !server_exe.is_file() {
        crate::set_service(app, |s| s.server = ServiceState::Failed);
        return Err(GameError::Message(format!(
            "{SERVER_EXE} is missing. Reinstall the components."
        )));
    }

    let mut server_cmd = launch::wrap_exe(app, &server_exe, &prefix_root, false)
        .map_err(|e| GameError::Message(e.to_string()))?;
    server_cmd
        .current_dir(&server_dir)
        // net8.0 reads every appsettings key from the environment, so our private
        // mongo port needs no file editing
        .env("DatabaseSettings__ConnectionString", &mongo_uri)
        .env("ASPNETCORE_ENVIRONMENT", "Production")
        .stdout(log_file(app, "server.log"))
        .stderr(log_file(app, "server.log"))
        .kill_on_drop(true);

    let server = server_cmd
        .spawn()
        .map_err(|e| GameError::Message(format!("Could not start the server: {e}")))?;
    down.server = Some(server);

    wait_for_server(app, &mut down).await?;
    crate::set_service(app, |s| s.server = ServiceState::Up);

    // 5 — the client loader.
    let win64 = settings::win64_dir(app);
    if !win64.join(STEAM_APPID_TXT).is_file() {
        log::warn!("{STEAM_APPID_TXT} is missing from Win64; restaging the components");
        crate::components::ensure(app)
            .await
            .map_err(|e| GameError::Message(e.to_string()))?;
    }
    let loader_exe = win64.join(LOADER_EXE);
    if !loader_exe.is_file() {
        return Err(GameError::Message(format!(
            "{LOADER_EXE} is missing. Reinstall the components."
        )));
    }

    let mut loader_cmd = launch::wrap_exe(app, &loader_exe, &prefix_root, true)
        .map_err(|e| GameError::Message(e.to_string()))?;
    loader_cmd
        .current_dir(&win64)
        .stdout(log_file(app, "loader.log"))
        .stderr(log_file(app, "loader.log"))
        .kill_on_drop(true);

    let loader = loader_cmd
        .spawn()
        .map_err(|e| GameError::Message(format!("Could not start the client loader: {e}")))?;
    down.loader = Some(loader);

    // 6 - wait for the game itself to appear, and keep its pid.
    let pid = wait_for_game(&dir, &mut down).await?;
    // the cached "is a game running" answer is now stale by construction
    proc::invalidate();

    if settings::discord_presence(app) {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        crate::presence::set_playing(since);
    }

    // 7 - wait for it to exit. the game is the authority, not the loader, which
    // usually exits as soon as it has injected. watching one pid rather than
    // rescanning: a full enumeration is ~55 ms and this loop runs for the session.
    let mut watch = proc::Watch::new(pid);
    while watch.alive() {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    log::info!("the game has exited");
    proc::invalidate();

    // 8 - teardown, on the way out of scope.
    Ok(0)
}

// any status counts: a 404 proves the pipeline is serving, a bound socket does not
async fn wait_for_server(app: &AppHandle, down: &mut Teardown<'_>) -> Result<(), GameError> {
    let url = format!("http://127.0.0.1:{}/", settings::SERVER_HTTP);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| GameError::Message(e.to_string()))?;

    let deadline = std::time::Instant::now() + SERVER_READY_TIMEOUT;
    loop {
        if let Some(child) = down.server.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                crate::set_service(app, |s| s.server = ServiceState::Failed);
                return Err(GameError::Message(format!(
                    "The server stopped with {status} before it began serving.\n\n{}",
                    crate::logs::tail(&settings::log_path(app, "server.log"), 20)
                )));
            }
        }
        if client.get(&url).send().await.is_ok() {
            log::info!("the server is serving on {url}");
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            crate::set_service(app, |s| s.server = ServiceState::Failed);
            return Err(GameError::Message(format!(
                "The server did not start within {}s.\n\n{}",
                SERVER_READY_TIMEOUT.as_secs(),
                crate::logs::tail(&settings::log_path(app, "server.log"), 20)
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

// returns the game's pid, so the caller can watch it without re-enumerating
async fn wait_for_game(dir: &Path, down: &mut Teardown<'_>) -> Result<sysinfo::Pid, GameError> {
    let deadline = std::time::Instant::now() + GAME_APPEAR_TIMEOUT;
    loop {
        if let Some(pid) = proc::find_game(dir) {
            log::info!("the game is running as pid {pid}");
            return Ok(pid);
        }
        // if the loader dies before the game appears, the full timeout tells
        // the user nothing useful
        if let Some(child) = down.loader.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                // re-check once: the loader normally exits the moment it injects
                if let Some(pid) = proc::find_game(dir) {
                    log::info!("the loader exited with {status}; the game is pid {pid}");
                    return Ok(pid);
                }
                return Err(GameError::Message(format!(
                    "The client loader exited with {status} without starting the game. \
                     Check the loader log in the Server tab."
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(GameError::Message(format!(
                "The game did not start within {}s. Check the loader log in the Server tab.",
                GAME_APPEAR_TIMEOUT.as_secs()
            )));
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

// user-initiated stop: kill everything running out of the install
pub fn stop(app: &AppHandle) {
    let dir = settings::game_directory(app);
    let killed = proc::kill_under(&dir) + proc::kill_under(&settings::server_dir(app));
    let swept = mongo::sweep_orphans(app);
    log::info!("stop: {killed} game/server process(es), {swept} mongod");
    crate::set_service(app, |s| {
        s.mongo = ServiceState::Down;
        s.server = ServiceState::Down;
        s.steam = ServiceState::Down;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("spc-game-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // a launcher update that ships a new blob must trigger a fresh pass
    #[test]
    fn depot_ok_requires_the_exact_manifest_id() {
        let dir = scratch("marker");
        assert!(!depot_ok(&dir, 42), "no marker means not installed");

        std::fs::write(dir.join(DEPOT_MARKER), "42").unwrap();
        assert!(depot_ok(&dir, 42));
        assert!(
            !depot_ok(&dir, 43),
            "a different build must not be accepted"
        );

        // a zero id is "we could not read the blob"; it must never validate
        std::fs::write(dir.join(DEPOT_MARKER), "0").unwrap();
        assert!(!depot_ok(&dir, 0));

        std::fs::write(dir.join(DEPOT_MARKER), "garbage").unwrap();
        assert!(!depot_ok(&dir, 42));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_on_disk_does_not_follow_symlinks() {
        let dir = scratch("size");
        std::fs::write(dir.join("a.bin"), vec![0u8; 1000]).unwrap();
        assert_eq!(size_on_disk(&dir), 1000);

        // a link to something huge outside the install must not be counted
        #[cfg(unix)]
        {
            let outside = dir.join("outside.bin");
            std::fs::write(&outside, vec![0u8; 5000]).unwrap();
            let nested = dir.join("nested");
            std::fs::create_dir_all(&nested).unwrap();
            std::os::unix::fs::symlink(&outside, nested.join("link.bin")).unwrap();
            // a.bin + outside.bin, but not the link's target counted twice
            assert_eq!(size_on_disk(&dir), 6000);
        }

        std::fs::remove_dir_all(&dir).ok();
    }
}
