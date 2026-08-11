// state, the busy gate, the event vocabulary, and the command surface

mod cert;
#[cfg(unix)]
mod compat;
mod components;
mod depot;
mod game;
mod launch;
mod logs;
mod mongo;
mod preflight;
mod presence;
mod proc;
mod settings;
mod state;
mod uninstall;

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};

pub use state::{Busy, Phase, ServiceState, Services};

// ---- state ----

#[derive(Default)]
pub struct AppState {
    busy: Option<Busy>,
    services: Services,
}

// a std mutex rather than tokio's: every access is a short field read or write
// with no await held across it, and BusyGuard::drop cannot await
#[derive(Default)]
pub struct Shared(Mutex<AppState>);

impl Shared {
    fn with<T>(&self, f: impl FnOnce(&mut AppState) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }
}

// releasing it is a Drop, not a manual call, so an early return cannot leave the
// launcher wedged
pub struct BusyGuard {
    app: AppHandle,
}

impl BusyGuard {
    // a single user action can pass through more than one stage. releasing the
    // claim between them would let a second click slip in, so only the label moves.
    pub fn relabel(&self, now: Busy) {
        self.app.state::<Shared>().with(|s| s.busy = Some(now));
        emit_phase(&self.app);
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.app.state::<Shared>().with(|s| s.busy = None);
        emit_phase(&self.app);
    }
}

// claim the launcher for want, or report what is already running
pub fn claim(app: &AppHandle, want: Busy) -> Result<BusyGuard, CommandError> {
    let taken = app.state::<Shared>().with(|s| match s.busy {
        Some(current) => Err(current),
        None => {
            s.busy = Some(want);
            Ok(())
        }
    });
    match taken {
        Err(current) => Err(CommandError::Busy {
            current: current.label(),
        }),
        Ok(()) => {
            emit_phase(app);
            Ok(BusyGuard { app: app.clone() })
        }
    }
}

// ---- errors ----

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{current} is already running.")]
    Busy { current: &'static str },
    #[error("Paused.")]
    Paused,
    #[error("Not available yet in this build.")]
    NotYet,
    #[error("{0}")]
    Message(String),
}

impl From<game::GameError> for CommandError {
    fn from(e: game::GameError) -> Self {
        match e {
            game::GameError::Paused => CommandError::Paused,
            other => CommandError::Message(other.to_string()),
        }
    }
}

// the frontend only shows the message, so serialise as the Display string
impl Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type CmdResult<T> = Result<T, CommandError>;

// ---- events ----

// (done, total, label). total == 0 means indeterminate.
pub fn progress(app: &AppHandle, done: u64, total: u64, label: &str) {
    let _ = tauri::Emitter::emit(app, "progress", (done, total, label));
}

pub fn show_bar(app: &AppHandle, show: bool) {
    let _ = tauri::Emitter::emit(app, "progressBar", show);
}

pub fn show_pause(app: &AppHandle, pausable: bool) {
    let _ = tauri::Emitter::emit(app, "progressPausable", pausable);
}

pub const NOTIFY_INFO: u8 = 0;
pub const NOTIFY_ERROR: u8 = 2;

// level: 0 info, 1 success, 2 error
pub fn notify(app: &AppHandle, text: &str, level: u8) {
    let _ = tauri::Emitter::emit(app, "notify", (text, level));
}

pub fn emit_phase(app: &AppHandle) {
    let phase = current_phase(app);
    let _ = tauri::Emitter::emit(app, "phase", phase);
}

pub fn set_service(app: &AppHandle, set: impl FnOnce(&mut Services)) {
    let services = app.state::<Shared>().with(|s| {
        set(&mut s.services);
        s.services
    });
    let _ = tauri::Emitter::emit(app, "services", services);
}

// ---- phase ----

// cached, but only a successful read: memoising the 0 from one transient failure
// pinned depot_ok false for the process and stuck the phase at NEEDS_GAME.
fn bundled_manifest_id(app: &AppHandle) -> u64 {
    static ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    if let Some(id) = ID.get() {
        return *id;
    }
    let read = depot::describe(app)
        .ok()
        .and_then(|d| d.manifest_id.parse().ok());
    match read {
        Some(id) => *ID.get_or_init(|| id),
        None => 0,
    }
}

// derived from disk on every call, never stored, so a stale marker cannot wedge
// the ui
fn current_phase(app: &AppHandle) -> Phase {
    if let Some(busy) = app.state::<Shared>().with(|s| s.busy) {
        return busy.into();
    }

    let dir = settings::game_directory(app);

    // the game may be running without us; Ready would offer a second launch.
    // cached, because enumerating costs ~55 ms on a 3-second poll.
    if proc::observe_cached(&dir).game {
        return Phase::Playing;
    }

    // the game cannot be launched without these, so they come first
    if !components::complete(app) {
        return Phase::NeedsComponents;
    }

    if game::depot_ok(&dir, bundled_manifest_id(app)) {
        return Phase::Ready;
    }
    // a half-written install offers Resume; an absent one offers Download
    if game::has_partial(&dir) {
        return Phase::Paused;
    }
    Phase::NeedsGame
}

// ---- snapshot ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub game_files: bool,
    pub components: bool,
    // a u64 as a string: this id exceeds javascript's safe integer range
    pub manifest_id: String,
    pub partial: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub phase: Phase,
    pub install: InstallState,
    pub services: Services,
    pub launcher_version: String,
    pub components_version: Option<String>,
    pub game_bytes: u64,
    pub free_bytes: u64,
    pub game_directory: String,
}

// ---- commands ----

// sizing the install walks 36 GiB of directory entries, so it runs off the ui thread
#[tauri::command]
async fn launcher_state(app: AppHandle) -> Snapshot {
    let dir = settings::game_directory(&app);
    let manifest_id = bundled_manifest_id(&app);
    let phase = current_phase(&app);

    // mongo and the server are ours, so their lamps come from what we started.
    // steam is not, so it is read from the same cached scan current_phase just used.
    let mut services = app.state::<Shared>().with(|s| s.services);
    if !matches!(phase, Phase::Starting | Phase::Playing) {
        services.steam = if proc::observe_cached(&dir).steam {
            state::ServiceState::Up
        } else {
            state::ServiceState::Down
        };
    }

    let probe = dir.clone();
    let (game_bytes, free_bytes) = tokio::task::spawn_blocking(move || {
        (game::size_on_disk(&probe), free_space(&probe).unwrap_or(0))
    })
    .await
    .unwrap_or((0, 0));

    Snapshot {
        phase,
        install: InstallState {
            game_files: game::depot_ok(&dir, manifest_id),
            components: components::complete(&app),
            manifest_id: manifest_id.to_string(),
            partial: game::has_partial(&dir),
        },
        services,
        launcher_version: app.package_info().version.to_string(),
        components_version: components::staged_version(&app),
        game_bytes,
        free_bytes,
        game_directory: dir.to_string_lossy().into_owned(),
    }
}

#[tauri::command]
fn log_tail(app: AppHandle, which: String, lines: usize) -> String {
    match which.as_str() {
        "mongod" => mongo::tail(&app, lines),
        "server" => logs::tail(&settings::log_path(&app, "server.log"), lines),
        "loader" => logs::tail(&settings::log_path(&app, "loader.log"), lines),
        "game" => logs::tail(&settings::log_path(&app, "game.log"), lines),
        _ => logs::tail(&settings::log_path(&app, "launcher.log"), lines),
    }
}

// the about tab's links. an allowlist because this is a webview handing the host
// a string to open: anything but http(s) is a scheme that runs something.
#[tauri::command]
fn open_link(url: String) -> CmdResult<()> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(CommandError::Message(format!("refusing to open {url}")));
    }
    open::that_detached(&url)
        .map_err(|e| CommandError::Message(format!("could not open {url}: {e}")))
}

#[tauri::command]
fn open_launcher_folder(app: AppHandle) -> CmdResult<()> {
    open_path(&settings::app_data(&app))
}

#[tauri::command]
fn open_game_folder(app: AppHandle) -> CmdResult<()> {
    open_path(&settings::game_directory(&app))
}

#[tauri::command]
async fn pick_game_directory(app: AppHandle) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let start = settings::game_directory(&app);
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose where the game files should live")
        .set_directory(&start)
        .pick_folder(move |picked| {
            let _ = tx.send(picked);
        });

    let picked = rx
        .await
        .map_err(|_| CommandError::Message("The folder picker closed unexpectedly.".into()))?;

    let Some(path) = picked else { return Ok(None) };
    let path = path.to_string();
    settings::set(
        &app,
        "game_directory",
        serde_json::Value::String(path.clone()),
    );
    emit_phase(&app);
    Ok(Some(path))
}

// declared here so the frontend's contract is complete and a mis-wired button
// fails loudly instead of silently

#[tauri::command]
async fn depot_info(app: AppHandle) -> CmdResult<depot::DepotInfo> {
    Ok(depot::describe(&app)?)
}

// walks every steam library and canonicalises each hit, so it runs off the pool
#[cfg(unix)]
#[tauri::command]
async fn detect_compat_tools() -> compat::CompatInfo {
    tokio::task::spawn_blocking(compat::detect)
        .await
        .unwrap_or_default()
}

#[cfg(windows)]
#[tauri::command]
async fn detect_compat_tools() -> serde_json::Value {
    serde_json::json!({ "supported": false, "proton": [], "runtimes": {}, "steamRoot": null })
}

#[tauri::command]
async fn preflight(app: AppHandle) -> preflight::Preflight {
    let handle = app.clone();
    tokio::task::spawn_blocking(move || preflight::run(&handle))
        .await
        .unwrap_or_else(|_| preflight::run(&app))
}

async fn stage_components(app: &AppHandle) -> CmdResult<()> {
    match components::ensure(app).await {
        Ok(true) => {
            notify(app, "Components installed.", 1);
            Ok(())
        }
        Ok(false) => Ok(()),
        Err(e) => {
            notify(app, &e.to_string(), 2);
            Err(CommandError::Message(e.to_string()))
        }
    }
}

#[tauri::command]
async fn install_components(app: AppHandle) -> CmdResult<()> {
    let _busy = claim(&app, Busy::Components)?;
    stage_components(&app).await
}

#[tauri::command]
async fn install_game(app: AppHandle) -> CmdResult<()> {
    // one claim for the whole action, relabelled as it progresses. claiming twice
    // would leave a gap in which a second press could start a competing pass.
    let needs_components = !components::complete(&app);
    let busy = claim(
        &app,
        if needs_components {
            Busy::Components
        } else {
            Busy::Downloading
        },
    )?;

    // components first: the game files are useless without something to launch them
    if needs_components {
        stage_components(&app).await?;
        busy.relabel(Busy::Downloading);
    }

    match game::download_game(&app).await {
        Ok(()) => {
            notify(&app, "The game is installed and verified.", 1);
            Ok(())
        }
        Err(game::GameError::Paused) => {
            notify(&app, "Download paused. Your progress is kept.", 0);
            Err(CommandError::Paused)
        }
        Err(e) => {
            notify(&app, &e.to_string(), 2);
            Err(e.into())
        }
    }
}

#[tauri::command]
async fn pause_download(app: AppHandle) -> CmdResult<()> {
    if depot::pause() {
        Ok(())
    } else {
        notify(&app, "Nothing is downloading right now.", 0);
        Err(CommandError::Message(
            "Nothing is downloading right now.".into(),
        ))
    }
}

#[tauri::command]
async fn verify_and_repair(app: AppHandle) -> CmdResult<()> {
    let _busy = claim(&app, Busy::Verifying)?;
    match game::verify_and_repair(&app).await {
        Ok(()) => {
            notify(&app, "All game files check out.", 1);
            Ok(())
        }
        Err(game::GameError::Paused) => Err(CommandError::Paused),
        Err(e) => {
            notify(&app, &e.to_string(), 2);
            Err(e.into())
        }
    }
}

#[tauri::command]
async fn play(app: AppHandle) -> CmdResult<i32> {
    // held for the whole session, so nothing can start a depot pass over files the
    // running game has open
    let _busy = claim(&app, Busy::Playing)?;
    match game::play(&app).await {
        Ok(code) => Ok(code),
        Err(e) => {
            notify(&app, &e.to_string(), 2);
            Err(e.into())
        }
    }
}

#[tauri::command]
async fn stop_game(app: AppHandle) -> CmdResult<()> {
    game::stop(&app);
    Ok(())
}

// walks the whole tree, so this runs off the ui thread
#[tauri::command]
async fn uninstall_plan(app: AppHandle) -> CmdResult<Vec<uninstall::Item>> {
    let handle = app.clone();
    tokio::task::spawn_blocking(move || uninstall::plan(&handle))
        .await
        .map_err(|e| CommandError::Message(e.to_string()))
}

#[tauri::command]
async fn uninstall_everything(app: AppHandle) -> CmdResult<()> {
    let _busy = claim(&app, Busy::Uninstalling)?;
    let failed = uninstall::execute(&app).await;
    if failed.is_empty() {
        notify(
            &app,
            "Everything this launcher installed has been removed.",
            1,
        );
        Ok(())
    } else {
        let msg = format!("Could not remove: {}.", failed.join(", "));
        notify(&app, &msg, 2);
        Err(CommandError::Message(msg))
    }
}

// ---- helpers ----

fn open_path(path: &std::path::Path) -> CmdResult<()> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|e| {
            CommandError::Message(format!("Could not create {}: {e}", path.display()))
        })?;
    }
    open::that_detached(path)
        .map_err(|e| CommandError::Message(format!("Could not open {}: {e}", path.display())))
}

// walks up to the nearest existing ancestor, because the game directory may not
// exist yet
pub fn free_space(path: &std::path::Path) -> Option<u64> {
    let mut probe = path;
    loop {
        if probe.exists() {
            break;
        }
        probe = probe.parent()?;
    }

    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| probe.starts_with(d.mount_point()))
        // the longest matching mount point is the one actually holding the path
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

// ---- run ----

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(Shared::default())
        .setup(|app| {
            let handle = app.handle().clone();
            std::fs::create_dir_all(settings::app_data(&handle)).ok();

            // a killed launcher leaves mongod on the dbpath lock or a server on 8443
            let swept =
                mongo::sweep_orphans(&handle) + proc::kill_under(&settings::server_dir(&handle));
            if swept > 0 {
                log::info!("cleared {swept} leftover process(es) from a previous run");
            }

            if let Err(e) = build_tray(&handle) {
                // a missing tray is a cosmetic loss, not a reason to refuse to start
                log::warn!("could not create the tray icon: {e}");
            }

            // without this, an unreadable blob shows up only as a download that
            // never completes
            match depot::describe(&handle) {
                Ok(d) => log::info!(
                    "depot {} manifest {} — {} files, {} to download",
                    d.depot_id,
                    d.manifest_id,
                    d.files,
                    depot::human(d.compressed_bytes)
                ),
                Err(e) => log::error!(
                    "the bundled depot manifest could not be read, so the game cannot be \
                     installed: {e}"
                ),
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            launcher_state,
            log_tail,
            open_launcher_folder,
            open_game_folder,
            open_link,
            pick_game_directory,
            depot_info,
            detect_compat_tools,
            preflight,
            install_components,
            install_game,
            pause_download,
            verify_and_repair,
            play,
            stop_game,
            uninstall_plan,
            uninstall_everything,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the launcher")
        .run(|handle, event| {
            // closing the window must not orphan a mongod or a server on 8443
            if let tauri::RunEvent::ExitRequested { .. } = event {
                game::stop(handle);
                presence::disconnect();
            }
        });
}

// a tray icon, so closing the window while the game runs does not lose the launcher
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or_else(|| tauri::Error::AssetNotFound("the bundled window icon".into()))?,
        )
        .tooltip("SPCycle Launcher")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            "quit" => {
                game::stop(app);
                presence::disconnect();
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}
