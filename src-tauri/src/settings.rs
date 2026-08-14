#![allow(dead_code)]

use std::path::PathBuf;

use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;

pub const STORE: &str = "storage.json";

// ports the sp server binds, from its appsettings.json
pub const SERVER_HTTP: u16 = 8000;
pub const SERVER_HTTPS: u16 = 8443;

// not 27017, so we never collide with a mongodb the user installed themselves
pub const MONGO_PORT_BASE: u16 = 27055;

// storage.json, read directly. split out so it can be tested without an AppHandle.
fn from_disk(path: &std::path::Path, key: &str) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()?
        .get(key)
        .cloned()
}

fn get(app: &AppHandle, key: &str) -> Option<serde_json::Value> {
    if let Ok(store) = app.store(STORE) {
        if let Some(v) = store.get(key) {
            return Some(v);
        }
    }
    from_disk(&app_data(app).join(STORE), key)
}

pub fn set(app: &AppHandle, key: &str, value: serde_json::Value) {
    if let Ok(store) = app.store(STORE) {
        store.set(key, value);
        // the store autosaves on a debounce, but a fresh setting should survive a crash
        let _ = store.save();
    }
}

fn string(app: &AppHandle, key: &str) -> Option<String> {
    match get(app, key)? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

fn bool_or(app: &AppHandle, key: &str, default: bool) -> bool {
    match get(app, key) {
        Some(serde_json::Value::Bool(b)) => b,
        _ => default,
    }
}

pub fn app_data(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|e| {
        log::error!("no platform data directory ({e}); falling back to the working directory");
        PathBuf::from(".")
    })
}

pub const LAUNCHER_LOG_STEM: &str = "launcher";

pub fn launcher_log(app: &AppHandle) -> PathBuf {
    app.path()
        .app_log_dir()
        .unwrap_or_else(|_| app_data(app))
        .join(format!("{LAUNCHER_LOG_STEM}.log"))
}

pub fn components_dir(app: &AppHandle) -> PathBuf {
    app_data(app).join("components")
}

// server, appsettings.json, generate_ssl.exe and the generated certificate.pfx
pub fn server_dir(app: &AppHandle) -> PathBuf {
    components_dir(app).join("server")
}

// the server's own prefix; the loader cannot patch through a shared one. see game::play.
pub fn server_prefix(app: &AppHandle) -> PathBuf {
    app_data(app).join("server-prefix")
}

pub fn mongo_dir(app: &AppHandle) -> PathBuf {
    app_data(app).join("mongodb")
}

pub fn mongo_exe(app: &AppHandle) -> PathBuf {
    let name = if cfg!(windows) {
        "mongod.exe"
    } else {
        "mongod"
    };
    mongo_dir(app).join("bin").join(name)
}

pub fn log_path(app: &AppHandle, name: &str) -> PathBuf {
    app_data(app).join(name)
}

pub fn game_log(app: &AppHandle) -> PathBuf {
    const TAIL: &str = "Prospect/Saved/Logs/Prospect.log";

    #[cfg(unix)]
    {
        game_directory(app)
            .join("compatdata/pfx/drive_c/users/steamuser/AppData/Local")
            .join(TAIL)
    }

    #[cfg(windows)]
    {
        let _ = app;
        match dirs::data_local_dir() {
            Some(local) => local.join(TAIL),
            None => PathBuf::from(TAIL),
        }
    }
}

// an explicit override, the bundled resource, then the source tree for dev builds
pub fn depot_blob(app: &AppHandle) -> PathBuf {
    if let Some(p) = string(app, "depot_blob_path") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return p;
        }
        log::warn!("depot_blob_path is set but not a file, falling back to the bundled blob");
    }

    let bundled = app
        .path()
        .resolve("depot.blob", tauri::path::BaseDirectory::Resource)
        .ok()
        .filter(|p| p.is_file());
    if let Some(p) = bundled {
        return p;
    }

    let in_tree = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("depot.blob");
    if in_tree.is_file() {
        log::debug!("using the depot blob from the source tree");
        return in_tree;
    }

    // return the resource path anyway, so the error names where it was expected
    app.path()
        .resolve("depot.blob", tauri::path::BaseDirectory::Resource)
        .unwrap_or(in_tree)
}

// the staged resource, then the source tree so `tauri dev` works from a checkout
#[cfg(unix)]
fn resource(app: &AppHandle, name: &str) -> PathBuf {
    let bundled = app
        .path()
        .resolve(
            format!("patch/{name}"),
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .filter(|p| p.is_file());
    if let Some(p) = bundled {
        return p;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join(name)
}

pub fn game_directory(app: &AppHandle) -> PathBuf {
    if let Some(p) = string(app, "game_directory") {
        return PathBuf::from(p);
    }
    default_game_directory(app)
}

fn default_game_directory(app: &AppHandle) -> PathBuf {
    #[cfg(windows)]
    {
        // local, not roaming: 37 GiB does not belong in a roaming profile
        if let Some(local) = dirs::data_local_dir() {
            return local.join(app.config().identifier.clone()).join("game");
        }
    }
    app_data(app).join("game")
}

// derived, not searched: the depot lays this out deterministically
pub fn win64_dir(app: &AppHandle) -> PathBuf {
    game_directory(app)
        .join("Prospect")
        .join("Binaries")
        .join("Win64")
}

// 0 means pick a free one at launch
pub fn mongo_port(app: &AppHandle) -> u16 {
    match get(app, "mongo_port") {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0) as u16,
        _ => 0,
    }
}

pub fn discord_presence(app: &AppHandle) -> bool {
    bool_or(app, "discord_presence", true)
}

pub fn autorun_steam(app: &AppHandle) -> bool {
    bool_or(app, "autorun_steam", true)
}

#[cfg(unix)]
pub fn proton_path(app: &AppHandle) -> Option<String> {
    string(app, "proton_path")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disk_fallback_reads_what_the_store_writes() {
        let dir = std::env::temp_dir().join("spcycle-settings-fallback");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STORE);

        assert_eq!(
            from_disk(&path, "proton_path"),
            None,
            "no file is not a panic"
        );

        std::fs::write(
            &path,
            r#"{"proton_path":"/x/proton","mongo_port":27055,"autorun_steam":false}"#,
        )
        .unwrap();
        assert_eq!(
            from_disk(&path, "proton_path").and_then(|v| v.as_str().map(String::from)),
            Some("/x/proton".into()),
            "the chosen proton must survive a store that is not answering"
        );
        assert_eq!(
            from_disk(&path, "mongo_port").and_then(|v| v.as_u64()),
            Some(27055)
        );
        assert_eq!(
            from_disk(&path, "autorun_steam").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(from_disk(&path, "missing"), None);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(
            from_disk(&path, "proton_path"),
            None,
            "a corrupt file is not a panic"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
