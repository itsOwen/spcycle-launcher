use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mongodb::bson::{doc, Document};
use mongodb::options::{ClientOptions, WriteConcern};
use mongodb::Client;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::state::ServiceState;
use crate::{mongo, proc, settings};

const DB: &str = "ProspectDb";
const COLL: &str = "PlayFabUserData";
const INVENTORY: &str = "Inventory";
const BALANCE: &str = "Balance";
const KEEP: usize = 10;

// the default is 30s, long enough that a dead mongod reads as a frozen button
const SELECTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum StashError {
    #[error(
        "The game is running. It keeps its own copy of the stash in memory and would \
         overwrite anything saved now, so close it first."
    )]
    GameRunning,
    #[error(
        "No saved character was found. Start the game once and let it reach the station, \
         then try again."
    )]
    NoProfile,
    #[error("That character is no longer in the database. Reload the stash and try again.")]
    ProfileGone,
    #[error(
        "The items were saved, but this character has no currency record for the launcher \
         to write to. Enter the station once and try again."
    )]
    NoBalanceRow,
    #[error("The stash was not saved: {0}")]
    Malformed(String),
    #[error("{0}")]
    Mongo(String),
    #[error("The database could not be read: {0}")]
    Db(String),
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for StashError {
    fn from(e: std::io::Error) -> Self {
        StashError::Io(e.to_string())
    }
}

impl From<mongo::MongoError> for StashError {
    fn from(e: mongo::MongoError) -> Self {
        StashError::Mongo(e.to_string())
    }
}

impl From<mongodb::error::Error> for StashError {
    fn from(e: mongodb::error::Error) -> Self {
        StashError::Db(e.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub playfab_id: String,
    pub items: String,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stash {
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub name: String,
    pub at: u64,
    pub bytes: u64,
    // shown in the list, because an empty backup and a broken one look identical without it
    pub items: u32,
}

// ---- helpers, split out so they test without an AppHandle ----

fn backup_dir(app: &AppHandle) -> PathBuf {
    settings::app_data(app).join("stash-backups")
}

// lexicographic order has to match chronological order: prune sorts by name
fn backup_name(at: SystemTime) -> String {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let rem = secs % 86_400;
    format!(
        "stash-{y:04}{m:02}{d:02}-{:02}{:02}{:02}.json",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// howard's civil_from_days. chrono is not a dependency and this is the only date we format
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// `name` crosses the ipc boundary, so it may not address anything but this directory
fn safe_name(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name.len() < 128
        && name.ends_with(".json")
        && !name.contains(['/', '\\', ':', '\0'])
        && name != ".."
        && !name.contains("..");
    ok.then_some(name)
}

// what the user typed, made safe to put on a filesystem
fn tidy_name(raw: &str) -> Option<String> {
    let stem = raw.trim().trim_end_matches(".json").trim();
    if stem.is_empty() {
        return None;
    }
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = format!("{}.json", cleaned.trim());
    safe_name(&name).map(str::to_string)
}

fn count_items(raw: &[u8]) -> u32 {
    let Ok(stash) = serde_json::from_slice::<Stash>(raw) else {
        return 0;
    };
    stash
        .profiles
        .iter()
        .filter_map(|p| serde_json::from_str::<Vec<serde_json::Value>>(&p.items).ok())
        .map(|v| v.len() as u32)
        .sum()
}

fn prune(dir: &Path, keep: usize) -> std::io::Result<()> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("stash-") && n.ends_with(".json"))
        .collect();
    if names.len() <= keep {
        return Ok(());
    }
    names.sort();
    for name in &names[..names.len() - keep] {
        std::fs::remove_file(dir.join(name))?;
    }
    Ok(())
}

// checked, never re-encoded: a corrupt string here is a save the game cannot load
fn check(items: &str, balance: Option<&str>) -> Result<(), StashError> {
    match serde_json::from_str::<serde_json::Value>(items) {
        Ok(serde_json::Value::Array(_)) => {}
        Ok(_) => {
            return Err(StashError::Malformed(
                "the item list is not an array".into(),
            ))
        }
        Err(e) => return Err(StashError::Malformed(e.to_string())),
    }
    if let Some(b) = balance {
        match serde_json::from_str::<serde_json::Value>(b) {
            Ok(serde_json::Value::Object(_)) => {}
            Ok(_) => return Err(StashError::Malformed("the balance is not an object".into())),
            Err(e) => return Err(StashError::Malformed(e.to_string())),
        }
    }
    Ok(())
}

// ---- database ----

fn refuse_if_playing(app: &AppHandle) -> Result<(), StashError> {
    // the cached answer is up to 10s old, and acting on a stale one sweeps a live session
    proc::invalidate();
    if proc::observe_cached(&settings::game_directory(app)).game {
        return Err(StashError::GameRunning);
    }
    Ok(())
}

async fn connect(port: u16) -> Result<Client, StashError> {
    let mut opts = ClientOptions::parse(format!("mongodb://127.0.0.1:{port}/")).await?;
    opts.server_selection_timeout = Some(SELECTION_TIMEOUT);

    opts.write_concern = Some(WriteConcern::builder().journal(true).build());
    Ok(Client::with_options(opts)?)
}

async fn read(client: &Client) -> Result<Stash, StashError> {
    let coll = client.database(DB).collection::<Document>(COLL);

    let mut profiles: Vec<Profile> = Vec::new();
    let mut cursor = coll.find(doc! { "Key": INVENTORY }).await?;
    while cursor.advance().await? {
        let row = cursor.deserialize_current()?;
        let (Ok(items), Ok(id)) = (row.get_str("Value"), row.get_str("PlayFabId")) else {
            log::warn!("skipping an Inventory row with no readable Value or PlayFabId");
            continue;
        };
        if id.is_empty() {
            continue;
        }
        profiles.push(Profile {
            playfab_id: id.to_string(),
            items: items.to_string(),
            balance: None,
        });
    }
    if profiles.is_empty() {
        return Err(StashError::NoProfile);
    }

    let mut cursor = coll.find(doc! { "Key": BALANCE }).await?;
    while cursor.advance().await? {
        let row = cursor.deserialize_current()?;
        let id = row.get_str("PlayFabId").unwrap_or_default();
        if let (Ok(value), Some(p)) = (
            row.get_str("Value"),
            profiles.iter_mut().find(|p| p.playfab_id == id),
        ) {
            p.balance = Some(value.to_string());
        }
    }

    Ok(Stash { profiles })
}

fn snapshot(app: &AppHandle, stash: &Stash) -> Result<String, StashError> {
    let dir = backup_dir(app);
    std::fs::create_dir_all(&dir)?;
    let name = backup_name(SystemTime::now());
    std::fs::write(
        dir.join(&name),
        serde_json::to_vec_pretty(stash).map_err(|e| StashError::Io(e.to_string()))?,
    )?;
    prune(&dir, KEEP)?;
    Ok(name)
}

async fn with_db<F, T>(app: &AppHandle, f: F) -> Result<T, StashError>
where
    F: AsyncFnOnce(&AppHandle, &Client) -> Result<T, StashError>,
{
    crate::set_service(app, |s| s.mongo = ServiceState::Starting);
    let (port, _) = match mongo::ensure(app).await {
        Ok(v) => v,
        Err(e) => {
            crate::set_service(app, |s| s.mongo = ServiceState::Failed);
            return Err(e.into());
        }
    };
    crate::set_service(app, |s| s.mongo = ServiceState::Up);

    let client = connect(port).await?;
    f(app, &client).await
}

// the tab opens the database; this is how the user closes it again
pub async fn stop_db(app: &AppHandle) -> Result<(), StashError> {
    refuse_if_playing(app)?;
    mongo::shutdown().await;
    crate::set_service(app, |s| s.mongo = ServiceState::Down);
    Ok(())
}

pub async fn load(app: &AppHandle) -> Result<Stash, StashError> {
    with_db(app, async |_app, client| read(client).await).await
}

pub async fn save(
    app: &AppHandle,
    playfab_id: &str,
    items: &str,
    balance: Option<&str>,
) -> Result<(), StashError> {
    check(items, balance)?;
    refuse_if_playing(app)?;

    with_db(app, async |app, client| {
        // backed up from what the database holds now, not from what the ui sent
        snapshot(app, &read(client).await?)?;

        let coll = client.database(DB).collection::<Document>(COLL);
        let hit = coll
            .update_one(
                doc! { "Key": INVENTORY, "PlayFabId": playfab_id },
                doc! { "$set": { "Value": items } },
            )
            .await?;
        if hit.matched_count == 0 {
            return Err(StashError::ProfileGone);
        }

        // ponytail: no upsert. a Balance row the server never wrote is an unknown.
        if let Some(b) = balance {
            let hit = coll
                .update_one(
                    doc! { "Key": BALANCE, "PlayFabId": playfab_id },
                    doc! { "$set": { "Value": b } },
                )
                .await?;
            // the items are already written, so this reports rather than fails the save
            if hit.matched_count == 0 {
                return Err(StashError::NoBalanceRow);
            }
        }
        Ok(())
    })
    .await
}

pub fn snapshot_now(
    app: &AppHandle,
    playfab_id: &str,
    items: &str,
    balance: Option<&str>,
) -> Result<String, StashError> {
    check(items, balance)?;
    let stash = Stash {
        profiles: vec![Profile {
            playfab_id: playfab_id.to_string(),
            items: items.to_string(),
            balance: balance.map(str::to_string),
        }],
    };
    snapshot(app, &stash)
}

// no database and no claim: these are file reads
pub fn backups(app: &AppHandle) -> Result<Vec<Backup>, StashError> {
    let dir = backup_dir(app);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out: Vec<Backup> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            safe_name(&name)?;
            let meta = e.metadata().ok()?;
            Some(Backup {
                items: count_items(&std::fs::read(e.path()).ok()?),
                name,
                at: meta
                    .modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()?
                    .as_secs(),
                bytes: meta.len(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(out)
}

pub fn delete_backup(app: &AppHandle, name: &str) -> Result<(), StashError> {
    let name = safe_name(name).ok_or_else(|| StashError::Io("no such backup.".into()))?;
    std::fs::remove_file(backup_dir(app).join(name))?;
    Ok(())
}

// a renamed backup also stops looking like one to prune, which is how a keeper is kept
pub fn rename_backup(app: &AppHandle, name: &str, to: &str) -> Result<String, StashError> {
    let from = safe_name(name).ok_or_else(|| StashError::Io("no such backup.".into()))?;
    let to = tidy_name(to).ok_or_else(|| StashError::Io("That name cannot be used.".into()))?;
    let dir = backup_dir(app);
    if to != from && dir.join(&to).exists() {
        return Err(StashError::Io(format!("{to} already exists.")));
    }
    std::fs::rename(dir.join(from), dir.join(&to))?;
    Ok(to)
}

pub fn read_backup(app: &AppHandle, name: &str) -> Result<Stash, StashError> {
    let name = safe_name(name).ok_or_else(|| StashError::Io("no such backup.".into()))?;
    let raw = std::fs::read(backup_dir(app).join(name))?;
    serde_json::from_slice(&raw).map_err(|e| StashError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("spc-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // prune must never drop a file newer than one it kept
    #[test]
    fn only_the_newest_ten_backups_survive_a_prune() {
        let dir = scratch("prune");
        for i in 1..=15 {
            std::fs::write(dir.join(format!("stash-202608{i:02}-000000.json")), "{}").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "keep me").unwrap();
        prune(&dir, KEEP).unwrap();

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left.len(), KEEP + 1, "{left:?}");
        assert!(left.contains(&"notes.txt".to_string()));
        assert!(left.contains(&"stash-20260815-000000.json".to_string()));
        assert!(!left.contains(&"stash-20260801-000000.json".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    // the opaque Value string must come back byte for byte, key order included
    #[test]
    fn a_backup_round_trips_its_item_json_byte_for_byte() {
        let dir = scratch("roundtrip");
        let items = r#"[{"itemId":"a","baseItemId":"WP_x","modData":{"m":[]},"amount":1}]"#;
        let stash = Stash {
            profiles: vec![Profile {
                playfab_id: "PF1".into(),
                items: items.into(),
                balance: Some(r#"{"AU":1,"SC":2,"IN":3}"#.into()),
            }],
        };
        let path = dir.join("stash-20260813-225800.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&stash).unwrap()).unwrap();

        let back: Stash = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.profiles[0].items, items);
        assert_eq!(
            back.profiles[0].balance.as_deref(),
            Some(r#"{"AU":1,"SC":2,"IN":3}"#)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // prune sorts by name, so the names have to order the same way the clock does
    #[test]
    fn backup_names_sort_chronologically() {
        let early = backup_name(UNIX_EPOCH + std::time::Duration::from_secs(1_775_000_000));
        let late = backup_name(UNIX_EPOCH + std::time::Duration::from_secs(1_775_086_400));
        assert!(early < late, "{early} !< {late}");
        assert_eq!(
            backup_name(UNIX_EPOCH + std::time::Duration::from_secs(1_775_000_000)),
            "stash-20260331-233320.json"
        );
    }

    // the name arrives from the webview, so it may not walk out of the directory
    #[test]
    fn a_backup_name_cannot_escape_its_directory() {
        assert!(safe_name("stash-20260813-225800.json").is_some());
        for bad in [
            "../../etc/passwd.json",
            "..",
            "/etc/shadow.json",
            "sub\\dir.json",
            "C:evil.json",
            "stash.txt",
            "",
        ] {
            assert!(safe_name(bad).is_none(), "{bad} was accepted");
        }
    }

    // a typed name reaches the filesystem, so it gets the same treatment as one we chose
    #[test]
    fn a_renamed_backup_cannot_escape_its_directory() {
        assert_eq!(tidy_name("my loadout").as_deref(), Some("my loadout.json"));
        assert_eq!(
            tidy_name("  raid kit.json  ").as_deref(),
            Some("raid kit.json")
        );
        assert_eq!(tidy_name("a/b\\c").as_deref(), Some("a_b_c.json"));
        // separators are replaced, but a name still carrying .. is refused outright
        assert!(tidy_name("../../etc/passwd").is_none());
        assert!(tidy_name("").is_none());
        assert!(tidy_name("   .json ").is_none());
    }

    // a keeper is a backup that no longer looks like one to prune
    #[test]
    fn a_renamed_backup_survives_pruning() {
        let dir = scratch("keeper");
        for i in 1..=12 {
            std::fs::write(dir.join(format!("stash-202608{i:02}-000000.json")), "{}").unwrap();
        }
        std::fs::write(dir.join("my loadout.json"), "{}").unwrap();
        prune(&dir, KEEP).unwrap();
        assert!(dir.join("my loadout.json").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    // the list is the only place a backup's contents are visible before loading it
    #[test]
    fn an_empty_backup_reports_no_items() {
        let full = br#"{"profiles":[{"playfabId":"a","items":"[{\"amount\":1},{\"amount\":2}]","balance":null}]}"#;
        assert_eq!(count_items(full), 2);
        assert_eq!(
            count_items(br#"{"profiles":[{"playfabId":"a","items":"[]","balance":null}]}"#),
            0
        );
        assert_eq!(count_items(b"not json"), 0);
    }

    // a write of anything but an array is a stash the game can no longer load
    #[test]
    fn a_write_is_refused_unless_items_are_an_array() {
        assert!(check("[]", None).is_ok());
        assert!(check(r#"[{"amount":1}]"#, Some(r#"{"AU":0}"#)).is_ok());
        assert!(check("{}", None).is_err());
        assert!(check("not json", None).is_err());
        assert!(check("[]", Some("[]")).is_err());
        assert!(check("[]", Some("oops")).is_err());
    }
}
