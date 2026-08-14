use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::AppHandle;

use crate::{cert, game, mongo, proc, settings};

const DEPOT_ENTRIES: &[&str] = &[
    "BattlEye",
    "Engine",
    "Movies",
    "Paks",
    "Prospect",
    "metadata",
    ".DepotDownloader",
    "Prospect_BE.exe",
    "installscript.vdf",
    "Manifest_DebugFiles_Win64.txt",
    "Manifest_NonUFSFiles_Win64.txt",
];

const LOADER_FILES: &[&str] = &[
    "Prospect.Client.Loader.exe",
    "Prospect.Agent.dll",
    "UE4SS.dll",
    "UE4SS-settings.ini",
    "MemberVariableLayout.ini",
    "steam_appid.txt",
    "Mods",
    "LICENSE",
    // ours, not the loader pack's: the client patch loader and its endpoint file
    "spcycle-inject.exe",
    "dwmapi.dll",
    "backend.txt",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub label: String,
    pub path: String,
    pub bytes: u64,
    // false when we cannot prove the thing is ours; the ui greys these out
    pub removable: bool,
    pub note: Option<String>,
}

enum Target {
    GameDir(PathBuf),
    LoaderFiles(PathBuf),
    // anything strictly inside the launcher's own data directory
    Owned(PathBuf),
    RootCert,
    Store,
}

// ours carries prime_prefix's stamp, or holds pfx directly; steam's compatdata never does
fn is_our_prefix(path: &Path) -> bool {
    path.join(".spc_proton").is_file() || path.join("pfx").is_dir()
}

// files this launcher put there itself, safe wherever the directory points
fn is_ours(dir: &Path, name: &str) -> bool {
    if name == "compatdata" {
        return is_our_prefix(&dir.join(name));
    }
    name == game::DEPOT_MARKER || name == game::DEPOT_PARTIAL || name == "compatdata.settings-stash"
}

fn is_game_entry(dir: &Path, name: &str) -> bool {
    is_ours(dir, name) || DEPOT_ENTRIES.contains(&name)
}

fn has_marker(dir: &Path) -> bool {
    [game::DEPOT_MARKER, game::DEPOT_PARTIAL]
        .iter()
        .any(|m| dir.join(m).is_file())
}

// true only when nothing unrecognised is in the directory
fn holds_only_the_game(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries {
        // an entry we cannot even read is one we cannot vouch for
        let Ok(entry) = entry else { return false };
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|n| is_game_entry(dir, n))
        {
            return false;
        }
    }
    true
}

fn our_entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| is_ours(dir, n)))
        .map(|e| e.path())
        .collect()
}

fn remove(path: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match remove_once(path) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && attempt < 3 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            other => return other,
        }
    }
}

fn remove_once(path: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().map(|e| size_of(&e.path())).sum()
}

// the only path that ever deletes anything in the game directory
fn remove_game_dir(dir: &Path) -> std::io::Result<()> {
    if !has_marker(dir) {
        return Ok(());
    }
    if holds_only_the_game(dir) {
        return remove(dir);
    }
    our_entries(dir).iter().try_for_each(|p| remove(p))?;
    // succeeds only if ours was all that was ever in there
    let _ = std::fs::remove_dir(dir);
    Ok(())
}

// removable wholesale only if it sits strictly inside our data dir
fn inside_app_data(app: &AppHandle, path: &Path) -> bool {
    let root = settings::app_data(app);
    path != root && path.starts_with(&root)
}

fn targets(app: &AppHandle) -> Vec<(Item, Target)> {
    let mut out = Vec::new();
    let game_dir = settings::game_directory(app);

    // --- the game files ---
    let marker = has_marker(&game_dir);
    let only_ours = marker && holds_only_the_game(&game_dir);
    out.push((
        Item {
            label: "Game files".into(),
            path: game_dir.display().to_string(),
            bytes: if marker { size_of(&game_dir) } else { 0 },
            removable: marker,
            note: if !marker {
                Some("No install by this launcher was found here.".into())
            } else if !only_ours {
                Some(
                    "This directory holds files this launcher did not put there, so only \
                     its own will be removed."
                        .into(),
                )
            } else {
                None
            },
        },
        Target::GameDir(game_dir.clone()),
    ));

    // --- loader files sitting in the game's own Win64 ---
    let win64 = settings::win64_dir(app);
    let present: Vec<&str> = LOADER_FILES
        .iter()
        .copied()
        .filter(|n| win64.join(n).exists())
        .collect();
    if !present.is_empty() {
        out.push((
            Item {
                label: "Client loader".into(),
                path: win64.display().to_string(),
                bytes: present.iter().map(|n| size_of(&win64.join(n))).sum(),
                removable: true,
                note: Some(format!("{} file(s), by name only.", present.len())),
            },
            Target::LoaderFiles(win64.clone()),
        ));
    }

    // --- things wholly inside our own data directory ---
    for (label, path) in [
        ("Components", settings::components_dir(app)),
        ("MongoDB", settings::mongo_dir(app)),
        (
            "Stash backups",
            settings::app_data(app).join("stash-backups"),
        ),
        ("Server prefix", settings::server_prefix(app)),
    ] {
        if !path.exists() {
            continue;
        }
        let owned = inside_app_data(app, &path);
        out.push((
            Item {
                label: label.into(),
                path: path.display().to_string(),
                bytes: size_of(&path),
                removable: owned,
                note: (!owned).then(|| {
                    "This is outside the launcher's data folder, so it is left alone.".into()
                }),
            },
            Target::Owned(path),
        ));
    }

    // --- the trusted certificate ---
    let thumb = cert::trusted_thumbprint(app);
    out.push((
        Item {
            label: "Trusted certificate".into(),
            path: thumb.clone().unwrap_or_else(|| "—".into()),
            bytes: 0,
            removable: thumb.is_some(),
            note: match &thumb {
                Some(_) => Some("Removed by thumbprint, nothing else is touched.".into()),
                None => Some("No certificate recorded by this launcher.".into()),
            },
        },
        Target::RootCert,
    ));

    // --- settings ---
    out.push((
        Item {
            label: "Launcher settings".into(),
            path: settings::app_data(app)
                .join(settings::STORE)
                .display()
                .to_string(),
            bytes: 0,
            removable: true,
            note: None,
        },
        Target::Store,
    ));

    out
}

// walks the install to size it, so callers run this off the ui thread
pub fn plan(app: &AppHandle) -> Vec<Item> {
    targets(app).into_iter().map(|(item, _)| item).collect()
}

// returns the labels of anything that could not be removed
pub async fn execute(app: &AppHandle) -> Vec<String> {
    // nothing may hold a file open while we delete it
    game::stop(app);

    // the walk that sizes the install, off the runtime
    let handle = app.clone();
    let plan = tokio::task::spawn_blocking(move || targets(&handle))
        .await
        .unwrap_or_default();

    let mut failed = Vec::new();
    for (item, target) in plan {
        if !item.removable {
            continue;
        }
        let ok = match target {
            // deleting 37 GiB would otherwise park a worker for minutes
            Target::GameDir(dir) => {
                tokio::task::spawn_blocking(move || remove_game_dir(&dir).is_ok())
                    .await
                    .unwrap_or(false)
            }

            Target::LoaderFiles(win64) => {
                LOADER_FILES
                    .iter()
                    .filter(|n| remove(&win64.join(n)).is_err())
                    .count()
                    == 0
            }
            Target::Owned(path) => {
                // re-checked here, not trusted from the plan
                inside_app_data(app, &path) && remove(&path).is_ok()
            }
            Target::RootCert => {
                let prefix_root = settings::game_directory(app);
                cert::untrust(app, &prefix_root).await.is_ok()
            }
            Target::Store => {
                let path = settings::app_data(app).join(settings::STORE);
                remove(&path).is_ok()
            }
        };
        if !ok {
            failed.push(item.label);
        }
    }

    let _ = mongo::sweep_orphans(app);
    let _ = proc::kill_under(&settings::app_data(app));
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spc-uninst-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_real_home_directory_survives_an_uninstall() {
        let home = scratch("home");

        // a plausible home directory
        for name in [
            "Documents",
            "Downloads",
            "Pictures",
            ".ssh",
            ".bashrc",
            ".config",
            "notes.txt",
        ] {
            if name.contains('.') && !name.starts_with('.') {
                std::fs::write(home.join(name), b"user data").unwrap();
            } else {
                std::fs::create_dir_all(home.join(name)).unwrap();
                std::fs::write(home.join(name).join("payload"), b"user data").unwrap();
            }
        }

        // ...that we also installed into
        std::fs::write(home.join(game::DEPOT_MARKER), b"123").unwrap();
        std::fs::create_dir_all(home.join("Prospect")).unwrap();
        std::fs::create_dir_all(home.join(".DepotDownloader")).unwrap();
        std::fs::create_dir_all(home.join("compatdata/pfx")).unwrap();

        assert!(has_marker(&home), "the marker is there — that is the trap");
        assert!(
            !holds_only_the_game(&home),
            "a home directory must never look like a bare install"
        );

        remove_game_dir(&home).unwrap();

        assert!(home.is_dir(), "the home directory itself must still exist");
        for name in [
            "Documents",
            "Downloads",
            "Pictures",
            ".ssh",
            ".bashrc",
            ".config",
            "notes.txt",
        ] {
            assert!(
                home.join(name).exists(),
                "{name} was deleted from a home directory"
            );
        }

        // ours went, the game's stayed
        assert!(!home.join(game::DEPOT_MARKER).exists());
        assert!(
            !home.join("compatdata").exists(),
            "our own prefix should go"
        );
        assert!(home.join("Prospect").is_dir());

        std::fs::remove_dir_all(&home).ok();
    }

    // a directory holding only the game and our markers is removed whole
    #[test]
    fn a_bare_install_is_removed_entirely() {
        let dir = scratch("bare");
        std::fs::write(dir.join(game::DEPOT_MARKER), b"123").unwrap();
        std::fs::create_dir_all(dir.join("Prospect/Binaries/Win64")).unwrap();
        std::fs::create_dir_all(dir.join("Engine")).unwrap();
        std::fs::write(dir.join("Prospect_BE.exe"), b"x").unwrap();

        assert!(holds_only_the_game(&dir));
        remove_game_dir(&dir).unwrap();
        assert!(!dir.exists(), "a bare install should be gone entirely");
    }

    // without a marker nothing is touched, however game-like it looks
    #[test]
    fn a_directory_without_a_marker_is_never_touched() {
        let dir = scratch("nomarker");
        std::fs::create_dir_all(dir.join("Prospect")).unwrap();
        std::fs::write(dir.join("Prospect_BE.exe"), b"x").unwrap();

        remove_game_dir(&dir).unwrap();
        assert!(dir.join("Prospect").is_dir(), "no marker means hands off");
        assert!(dir.join("Prospect_BE.exe").is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn steams_shared_compatdata_is_never_mistaken_for_ours() {
        let dir = scratch("steam");
        let steam_compat = dir.join("compatdata");
        for appid in ["1091500", "570", "440"] {
            std::fs::create_dir_all(steam_compat.join(appid).join("pfx")).unwrap();
        }

        assert!(
            !is_our_prefix(&steam_compat),
            "steam's compatdata must not look like ours"
        );
        assert!(!is_ours(&dir, "compatdata"));

        // ours, by either proof
        let ours = scratch("ourprefix");
        std::fs::create_dir_all(ours.join("compatdata/pfx")).unwrap();
        assert!(is_our_prefix(&ours.join("compatdata")));

        let stamped = scratch("stamped");
        std::fs::create_dir_all(stamped.join("compatdata")).unwrap();
        std::fs::write(stamped.join("compatdata/.spc_proton"), b"").unwrap();
        assert!(is_our_prefix(&stamped.join("compatdata")));

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&ours).ok();
        std::fs::remove_dir_all(&stamped).ok();
    }

    // a symlink out of the install must not be followed when sizing or deleting
    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed() {
        let dir = scratch("links");
        let precious = scratch("precious");
        std::fs::write(precious.join("keepme"), vec![0u8; 4096]).unwrap();

        std::fs::write(dir.join(game::DEPOT_MARKER), b"1").unwrap();
        std::os::unix::fs::symlink(&precious, dir.join("Prospect")).unwrap();

        // sizing must not walk into the link target
        assert_eq!(size_of(&dir.join("Prospect")), 0);

        remove_game_dir(&dir).unwrap();
        assert!(
            precious.join("keepme").is_file(),
            "a symlinked directory's contents must survive"
        );

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&precious).ok();
    }

    // an unreadable entry means we cannot vouch for the directory
    #[test]
    fn an_unreadable_directory_is_not_claimed() {
        let missing = std::env::temp_dir().join("spc-does-not-exist-uninst");
        assert!(!holds_only_the_game(&missing));
        assert!(our_entries(&missing).is_empty());
    }

    #[test]
    #[ignore = "reads resources/depot.blob; run tools/fetch-depot-blob.sh first"]
    fn no_loader_file_shares_a_name_with_a_depot_file() {
        let blob = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/depot.blob");
        let bytes = std::fs::read(&blob).expect("run tools/fetch-depot-blob.sh");
        let manifest = crate::depot::manifest_for_tests(&bytes).expect("the blob parses");

        for entry in &manifest.files {
            let leaf = entry
                .filename
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(&entry.filename);
            for ours in LOADER_FILES {
                assert!(
                    !leaf.eq_ignore_ascii_case(ours),
                    "the depot ships {leaf:?} ({}), which uninstall would delete by name",
                    entry.filename
                );
            }
        }
        println!(
            "checked {} loader names against {} depot files — no collisions",
            LOADER_FILES.len(),
            manifest.files.len()
        );
    }

    #[test]
    fn loader_files_are_a_closed_list() {
        // if this grows, uninstall must grow with it, or files are orphaned
        assert!(LOADER_FILES.contains(&"Prospect.Client.Loader.exe"));
        assert!(LOADER_FILES.contains(&"steam_appid.txt"));
        assert!(
            !LOADER_FILES.contains(&"Prospect-Win64-Shipping.exe"),
            "the game's own executable is not ours to delete"
        );
    }
}
