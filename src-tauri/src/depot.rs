// native depot downloading, in place of shelling out to DepotDownloader.exe.
//
// depot.blob is a zlib stream whose first 32 bytes are the depot key and whose
// remainder is a steam depot manifest. steam itself is touched only for the cdn
// server list, which an anonymous logon may ask for: no account, no ownership,
// no manifest request code. chunks are fetched unauthenticated and decrypted
// locally with that key.
//
// every pass runs with verify(true), so install, repair and resume are one thing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use steamroom::cdn::server::CdnServer;
use steamroom::cdn::{CdnClient, CdnServerPool};
use steamroom::depot::manifest::DepotManifest;
use steamroom::depot::{CellId, DepotKey};
use steamroom_client::download::{CdnChunkFetcher, DepotJob};
use steamroom_client::event::DownloadEvent;
use steamroom_client::login::LoginBuilder;
use tauri::AppHandle;
use tokio::sync::Notify;

use crate::game::GameError;
use crate::{free_space, progress, settings};

// the key is the first 32 bytes of the inflated blob, the manifest the rest
const DEPOT_KEY_LEN: usize = 32;

// so a wrong or hostile blob cannot make us buffer without bound
const MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024;

// a handful is plenty to rotate between and keeps the logon response small
const MAX_CDN_SERVERS: u32 = 20;

// cell 0 is globally routable. we cannot learn the user's real cell without a
// steam install, and a suboptimal cell costs latency, not correctness.
const CELL_ID: CellId = CellId(0);

// the frontend cannot show more than a few a second anyway
const EMIT_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

// set while a pass is in flight, so pause can tell running from idle
static CANCEL: Mutex<Option<Arc<Notify>>> = Mutex::new(None);

// tells a download the user stopped from one that failed
static PAUSED: AtomicBool = AtomicBool::new(false);

// what the bundled blob describes, without downloading anything
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepotInfo {
    pub depot_id: u32,
    // a u64 as a string: this id exceeds javascript's safe integer range
    pub manifest_id: String,
    pub files: usize,
    // what lands on disk
    pub total_bytes: u64,
    // what actually crosses the network, far smaller than total_bytes
    pub compressed_bytes: u64,
    pub created_at: u32,
}

// false when nothing is running, which the caller reports rather than lying
pub fn pause() -> bool {
    let Some(cancel) = CANCEL.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    PAUSED.store(true, Ordering::SeqCst);
    // notify_one leaves a permit when nothing is waiting yet, so a pause that
    // races the start of the download is not lost. the Notify is per-pass.
    cancel.notify_one();
    true
}

// clears the global however the pass ends, so a later pause cannot address a
// download that has already finished
struct CancelGuard;

impl Drop for CancelGuard {
    fn drop(&mut self) {
        *CANCEL.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i < 2 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

// read the bundled blob and split it into the depot key and the manifest
fn load_depot(app: &AppHandle) -> Result<(DepotKey, DepotManifest), GameError> {
    let path = settings::depot_blob(app);
    let bytes = std::fs::read(&path).map_err(|e| {
        GameError::Message(format!(
            "Could not read the depot manifest at {}: {e}",
            path.display()
        ))
    })?;
    split_depot_blob(&bytes)
}

fn split_depot_blob(compressed: &[u8]) -> Result<(DepotKey, DepotManifest), GameError> {
    use std::io::Read;

    let mut blob = Vec::new();
    flate2::read::ZlibDecoder::new(compressed)
        .take(MAX_BLOB_BYTES)
        .read_to_end(&mut blob)
        .map_err(|e| {
            GameError::Message(format!("The depot manifest could not be decompressed: {e}"))
        })?;

    if blob.len() <= DEPOT_KEY_LEN {
        return Err(GameError::Message(
            "The depot manifest is too short to contain a depot key.".into(),
        ));
    }
    let (key, rest) = blob.split_at(DEPOT_KEY_LEN);
    let key = DepotKey(key.try_into().expect("split_at guarantees the length"));

    let mut manifest = DepotManifest::parse(rest)
        .map_err(|e| GameError::Message(format!("The depot manifest could not be parsed: {e}")))?;

    // this build ships unencrypted filenames, but the flag is authoritative
    if manifest.filenames_encrypted {
        manifest.decrypt_filenames(&key).map_err(|e| {
            GameError::Message(format!("The depot filenames could not be decrypted: {e}"))
        })?;
    }
    Ok((key, manifest))
}

// for tests in other modules that need to check themselves against the file list
#[cfg(test)]
pub fn manifest_for_tests(bytes: &[u8]) -> Result<DepotManifest, GameError> {
    split_depot_blob(bytes).map(|(_, m)| m)
}

// no network, no steam
pub fn describe(app: &AppHandle) -> Result<DepotInfo, GameError> {
    let (_, manifest) = load_depot(app)?;
    Ok(DepotInfo {
        depot_id: manifest.depot_id.map(|d| d.0).unwrap_or(0),
        manifest_id: manifest.manifest_id.map(|m| m.0).unwrap_or(0).to_string(),
        files: manifest.files.len(),
        total_bytes: manifest.total_uncompressed_size.unwrap_or(0),
        compressed_bytes: manifest.total_compressed_size.unwrap_or(0),
        created_at: manifest.creation_time.unwrap_or(0),
    })
}

// anonymous logon purely for the cdn server list. nothing about the account is
// used or kept, and the connection is dropped as soon as the list is in hand.
async fn cdn_servers() -> Result<Vec<CdnServer>, GameError> {
    let client = LoginBuilder::new()
        .cell_id(CELL_ID.0)
        .anonymous()
        .login()
        .await
        .map_err(|e| GameError::Interrupted(format!("could not reach Steam: {e}")))?;

    let servers = client
        .get_cdn_servers(CELL_ID, Some(MAX_CDN_SERVERS))
        .await
        .map_err(|e| {
            GameError::Interrupted(format!("Steam did not return a content server list: {e}"))
        })?;

    if servers.is_empty() {
        return Err(GameError::Interrupted(
            "Steam returned an empty content server list".into(),
        ));
    }
    log::info!("using {} Steam content servers", servers.len());
    Ok(servers)
}

// translates DownloadEvents into the progress tuple the frontend consumes.
//
// DepotProgress only arrives after a file is fetched, so a verify pass over
// already-good files would sit still without the FileSkipped accounting here.
// chunk bytes cover the opposite case: a multi-gigabyte pak.
struct Pump<'a> {
    app: &'a AppHandle,
    // shown until the job reaches its first file
    label: &'a str,
    // derived from the events, not the entry point, so a repair inside a verify
    // pass says Downloading for the files it actually refetches
    verb: &'static str,
    sizes: HashMap<String, u64>,
    total: u64,
    // bytes attributable to files the job has finished with
    settled: u64,
    // bytes of the in-flight file, reset when it lands
    in_flight: u64,
    current: String,
    last_emit: std::time::Instant,
}

impl<'a> Pump<'a> {
    fn new(app: &'a AppHandle, label: &'a str, manifest: &DepotManifest) -> Self {
        Self {
            app,
            label,
            verb: "Checking",
            sizes: manifest
                .files
                .iter()
                .map(|f| (f.filename.clone(), f.size))
                .collect(),
            total: 0,
            settled: 0,
            in_flight: 0,
            current: String::new(),
            last_emit: std::time::Instant::now() - EMIT_EVERY,
        }
    }

    fn base_name(filename: &str) -> &str {
        filename.rsplit(['\\', '/']).next().unwrap_or(filename)
    }

    fn handle(&mut self, event: DownloadEvent) {
        let mut force = false;
        match event {
            DownloadEvent::DownloadStarted {
                total_bytes,
                total_files,
            } => {
                log::info!(
                    "depot pass covers {total_files} files, {}",
                    human(total_bytes)
                );
                self.total = total_bytes;
                force = true;
            }
            DownloadEvent::FileStarted { ref filename } => {
                self.current = Self::base_name(filename).to_string();
                self.verb = "Downloading";
                self.in_flight = 0;
                force = true;
            }
            DownloadEvent::ChunkCompleted { bytes } => {
                self.in_flight = self.in_flight.saturating_add(bytes);
            }
            // a skip advances the job's byte count but emits no DepotProgress
            DownloadEvent::FileSkipped { ref filename } => {
                self.settled = self
                    .settled
                    .saturating_add(self.sizes.get(filename).copied().unwrap_or(0));
                self.in_flight = 0;
                self.current = Self::base_name(filename).to_string();
                self.verb = "Verifying";
            }
            // authoritative: the job's own total, which folds in every skip so far
            DownloadEvent::DepotProgress {
                completed_bytes,
                total_bytes,
            } => {
                self.settled = completed_bytes;
                self.in_flight = 0;
                self.total = total_bytes;
            }
            DownloadEvent::ChunkFailed { ref error } => {
                // the job retries internally, so this is a note, not a failure
                log::warn!("chunk fetch failed, retrying: {error}");
            }
            _ => {}
        }

        if !force && self.last_emit.elapsed() < EMIT_EVERY {
            return;
        }
        self.last_emit = std::time::Instant::now();
        // a zero total is the indeterminate phase before the job has sized the work
        let done = match self.total {
            0 => 0,
            total => self.settled.saturating_add(self.in_flight).min(total),
        };
        let text = if self.current.is_empty() {
            self.label.to_string()
        } else {
            format!("{} {}", self.verb, self.current)
        };
        progress(self.app, done, self.total, &text);
    }
}

// runs one full pass over dir. returns Paused when the user stopped it, leaving
// the partial install to resume from, or the manifest id on success.
pub async fn run(app: &AppHandle, dir: &Path, label: &str) -> Result<u64, GameError> {
    let cancel = Arc::new(Notify::new());
    PAUSED.store(false, Ordering::SeqCst);
    *CANCEL.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());
    let _guard = CancelGuard;

    progress(
        app,
        0,
        0,
        &format!("{label}: reading the depot manifest..."),
    );
    let (key, manifest) = load_depot(app)?;

    let depot_id = manifest
        .depot_id
        .ok_or_else(|| GameError::Message("The depot manifest does not name a depot.".into()))?;
    let manifest_id = manifest.manifest_id.map(|m| m.0).unwrap_or(0);
    log::info!(
        "depot {} manifest {} — {} files, {} uncompressed",
        depot_id.0,
        manifest_id,
        manifest.files.len(),
        human(manifest.total_uncompressed_size.unwrap_or(0)),
    );

    check_space(dir, &manifest)?;
    std::fs::create_dir_all(dir)
        .map_err(|e| GameError::Message(format!("Could not create {}: {e}", dir.display())))?;

    progress(
        app,
        0,
        0,
        &format!("{label}: connecting to the content servers..."),
    );
    // cancellable too: the logon can sit for a while on a bad network
    let servers = tokio::select! {
        biased;
        _ = cancel.notified() => return Err(GameError::Paused),
        r = cdn_servers() => r?,
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let job = DepotJob::builder()
        .depot_id(depot_id)
        .depot_key(key)
        .install_dir(dir.to_path_buf())
        // always on: it is what makes an interrupted pass resumable
        .verify(true)
        .event_sender(tx)
        .build()
        .map_err(|e| GameError::Message(format!("Could not prepare the download: {e}")))?;

    let fetcher = Arc::new(CdnChunkFetcher::new(
        CdnClient::new()
            .map_err(|e| GameError::Message(format!("Could not prepare the CDN client: {e}")))?
            // free when there is no lancache, and used when there is
            .with_lancache(),
        CdnServerPool::new(servers),
        // no cdn auth token: we hold the depot key, so chunks need no account
        None,
    ));

    let mut pump = Pump::new(app, label, &manifest);

    // the blocking pool, not a worker: a verify-only pass never awaits at all, it
    // hashes every file in one uninterrupted poll. on a worker that starves this
    // loop, because the task a worker wakes goes to its lifo slot, which no other
    // worker may steal, so nothing would be pumped or paused until the pass ended.
    // block_on keeps the network awaits inside working.
    // ponytail: abort() cannot interrupt a blocking task, so pause frees the ui
    // but the pass in flight runs itself out.
    let rt = tokio::runtime::Handle::current();
    let mut download = tokio::task::spawn_blocking(move || {
        rt.block_on(job.download(&manifest, fetcher)).map(|stats| {
            log::info!(
                "depot pass finished: {} files written, {} skipped, {} removed, {} fetched",
                stats.files_completed,
                stats.files_skipped,
                stats.files_removed,
                human(stats.bytes_downloaded),
            );
        })
    });

    let result = loop {
        tokio::select! {
            biased;
            Some(event) = rx.recv() => pump.handle(event),
            _ = cancel.notified() => {
                // stops it if it has not started; the ui is freed either way
                download.abort();
                break Err(GameError::Paused);
            }
            done = &mut download => {
                break match done {
                    Ok(r) => r.map_err(|e| GameError::Interrupted(e.to_string())),
                    Err(e) if e.is_cancelled() => Err(GameError::Paused),
                    Err(e) => Err(GameError::Interrupted(e.to_string())),
                };
            }
        }
    };

    // the chunk tasks it spawned are detached and may still be writing, so give
    // them a moment before another pass starts over the same directory.
    // ponytail: fixed grace rather than a join; the crate does not expose one.
    if matches!(result, Err(GameError::Paused)) {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    if PAUSED.swap(false, Ordering::SeqCst) {
        return Err(GameError::Paused);
    }
    result.map(|()| manifest_id)
}

// what the pass still has to write: the manifest total less what is on disk.
// discounting what is present is the point, or a resume would be refused on
// exactly the disks where the install is already nearly complete.
fn remaining_bytes(dir: &Path, manifest: &DepotManifest) -> Option<u64> {
    let total = manifest.total_uncompressed_size?;
    let have: u64 = manifest
        .files
        .iter()
        .filter_map(|f| std::fs::metadata(dir.join(f.normalized_path())).ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum();
    Some(total.saturating_sub(have))
}

fn check_space(dir: &Path, manifest: &DepotManifest) -> Result<(), GameError> {
    let Some(needed) = remaining_bytes(dir, manifest) else {
        return Ok(());
    };
    match free_space(dir) {
        Some(free) if free < needed => Err(GameError::NoSpace {
            needed: human(needed),
            available: human(free),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // pinned, so a bad fetch-depot-blob.sh run cannot silently swap the build
    const DEPOT_ID: u32 = 868271;
    const MANIFEST_ID: &str = "4623363103423775682";
    const FILES: usize = 412;
    const TOTAL_BYTES: u64 = 39_511_307_208;
    const COMPRESSED_BYTES: u64 = 16_231_331_264;

    fn blob_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/depot.blob")
    }

    #[test]
    fn a_blob_shorter_than_the_key_is_refused() {
        let short = deflate(&[0u8; 16]);
        assert!(split_depot_blob(&short).is_err());
    }

    #[test]
    fn a_blob_that_is_not_zlib_is_refused() {
        assert!(split_depot_blob(b"not compressed at all").is_err());
    }

    // proves the manifest is read from past the key: a manifest at offset 0 must
    // fail, because the parser is handed 32 bytes of it as the key
    #[test]
    fn the_manifest_is_read_from_after_the_key() {
        let real = std::fs::read(blob_path()).expect("run tools/fetch-depot-blob.sh");
        let (_, manifest) = split_depot_blob(&real).expect("the committed blob parses");
        assert!(manifest.depot_id.is_some());

        // same bytes, but with the key stripped so the manifest starts at 0
        let mut inflated = Vec::new();
        {
            use std::io::Read;
            flate2::read::ZlibDecoder::new(&real[..])
                .read_to_end(&mut inflated)
                .unwrap();
        }
        let shifted = deflate(&inflated[DEPOT_KEY_LEN..]);
        assert!(
            split_depot_blob(&shifted).is_err(),
            "a manifest at offset 0 must not parse, or the key offset is wrong"
        );
    }

    #[test]
    fn base_name_takes_the_leaf_of_a_windows_path() {
        assert_eq!(
            Pump::base_name("Prospect\\Content\\Paks\\pakchunk0.pak"),
            "pakchunk0.pak"
        );
        assert_eq!(Pump::base_name("Prospect/Content/x.uasset"), "x.uasset");
        assert_eq!(Pump::base_name("loose.txt"), "loose.txt");
    }

    #[test]
    fn pause_reports_when_nothing_is_running() {
        assert!(
            !pause(),
            "pause must not claim success with no pass in flight"
        );
    }

    #[test]
    fn remaining_shrinks_as_files_appear() {
        let real = std::fs::read(blob_path()).expect("run tools/fetch-depot-blob.sh");
        let (_, manifest) = split_depot_blob(&real).unwrap();

        let dir = std::env::temp_dir().join(format!("spc-depot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let empty = remaining_bytes(&dir, &manifest).unwrap();
        assert_eq!(empty, manifest.total_uncompressed_size.unwrap());

        // materialise one real file at its true size
        let first = manifest
            .files
            .iter()
            .find(|f| f.size > 0)
            .expect("the manifest has a non-empty file");
        let path = dir.join(first.normalized_path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![0u8; first.size as usize]).unwrap();

        let after = remaining_bytes(&dir, &manifest).unwrap();
        assert_eq!(after, empty - first.size);

        std::fs::remove_dir_all(&dir).ok();
    }

    // ignored by default only because it needs the blob on disk; downloads nothing
    #[test]
    #[ignore = "needs resources/depot.blob; run tools/fetch-depot-blob.sh first"]
    fn the_bundled_blob_is_the_frozen_depot() {
        let real = std::fs::read(blob_path()).expect("run tools/fetch-depot-blob.sh");
        let (_, m) = split_depot_blob(&real).expect("the committed blob parses");

        let depot_id = m.depot_id.unwrap().0;
        let manifest_id = m.manifest_id.unwrap().0.to_string();
        let total = m.total_uncompressed_size.unwrap();
        let compressed = m.total_compressed_size.unwrap();

        println!(
            "depot {depot_id}, manifest {manifest_id}, {} files\n  download {} / on disk {}\n  built {}",
            m.files.len(),
            human(compressed),
            human(total),
            m.creation_time.unwrap_or(0),
        );

        assert_eq!(depot_id, DEPOT_ID);
        assert_eq!(manifest_id, MANIFEST_ID);
        assert_eq!(m.files.len(), FILES);
        assert_eq!(total, TOTAL_BYTES);
        assert_eq!(compressed, COMPRESSED_BYTES);
        assert!(!m.filenames_encrypted, "this build ships plain filenames");
    }

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    // download exactly one file by name, shared by the two live tests below
    fn fetch_one(
        dir: &std::path::Path,
        key: &DepotKey,
        manifest: &DepotManifest,
        filename: &str,
    ) -> steamroom_client::download::DownloadStats {
        use steamroom_client::download::{CdnChunkFetcher, DepotJob, FileFilter};

        let depot_id = manifest.depot_id.unwrap();
        let filter = FileFilter::from_filelist(&[filename.to_string()]).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let servers = cdn_servers().await.expect("anonymous logon + server list");
            let fetcher = Arc::new(CdnChunkFetcher::new(
                CdnClient::new().unwrap().with_lancache(),
                CdnServerPool::new(servers),
                None,
            ));
            DepotJob::builder()
                .depot_id(depot_id)
                .depot_key(key.clone())
                .install_dir(dir.to_path_buf())
                .verify(true)
                .file_filter(filter)
                .build()
                .unwrap()
                .download(manifest, fetcher)
                .await
                .expect("download")
        })
    }

    // settles which tls backend the game's http stack uses, which decides whether
    // trusting the local server's certificate is possible at all.
    //
    //     RUN_LIVE=1 cargo test --lib -- --ignored --nocapture probe_tls
    #[test]
    #[ignore = "downloads the 110 MB game executable; set RUN_LIVE=1"]
    fn live_probe_tls_backend_of_the_shipping_exe() {
        if std::env::var("RUN_LIVE").is_err() {
            eprintln!("skipping: set RUN_LIVE=1 to run this");
            return;
        }

        const EXE: &str = r"Prospect\Binaries\Win64\Prospect-Win64-Shipping.exe";

        let real = std::fs::read(blob_path()).expect("run tools/fetch-depot-blob.sh");
        let (key, manifest) = split_depot_blob(&real).unwrap();

        // no cacert.pem anywhere in the manifest is itself a finding
        let bundles: Vec<&str> = manifest
            .files
            .iter()
            .map(|f| f.filename.as_str())
            .filter(|n| n.to_lowercase().contains("cacert") || n.to_lowercase().ends_with(".pem"))
            .collect();
        println!("CA bundles shipped by the depot: {bundles:?}");
        assert!(
            bundles.is_empty(),
            "a bundled CA file changes the certificate-trust design; investigate"
        );

        // a stable directory: verify(true) makes a re-run skip the download
        let dir = std::env::temp_dir().join("spcycle-tls-probe");
        std::fs::create_dir_all(&dir).unwrap();

        println!("fetching {EXE} (110 MB, cached across runs)...");
        let stats = fetch_one(&dir, &key, &manifest, EXE);
        println!(
            "  {} written, {} skipped",
            stats.files_completed, stats.files_skipped
        );

        let path = dir.join(EXE.replace('\\', "/"));
        let bytes = std::fs::read(&path).expect("the exe landed");
        println!("  {} on disk", human(bytes.len() as u64));

        // crude but sufficient: look for the marker strings each backend leaves
        let count = |needle: &str| {
            let n = needle.as_bytes();
            bytes.windows(n.len()).filter(|w| *w == n).count()
        };
        println!("\n  -- TLS backend --");
        for needle in [
            "schannel", "Schannel", "SCHANNEL", "OpenSSL", "openssl", "libcurl", "PEM_read",
        ] {
            let n = count(needle);
            if n > 0 {
                println!("  {n:>5} x {needle:?}");
            }
        }

        // the decisive question. ue4's ssl module uses openssl for the crypto but
        // can seed its trust store from the windows certificate store via crypt32.
        // if those imports are present, the current-user root store is the target,
        // and on linux wine's crypt32 implementation of the same calls.
        println!("\n  -- windows certificate store (crypt32) --");
        let mut store_api = 0;
        for needle in [
            "CertOpenSystemStore",
            "CertOpenStore",
            "CertEnumCertificatesInStore",
            "CertCloseStore",
            "CertFreeCertificateContext",
            "crypt32.dll",
            "CRYPT32.dll",
        ] {
            let n = count(needle);
            if n > 0 {
                store_api += n;
                println!("  {n:>5} x {needle:?}");
            }
        }
        if store_api == 0 {
            println!("  (none)");
        }

        println!("\n  -- PEM bundle paths --");
        for needle in [
            "cacert.pem",
            "Certificates",
            "ThirdParty/cacert",
            "ca-bundle",
        ] {
            let n = count(needle);
            if n > 0 {
                println!("  {n:>5} x {needle:?}");
            }
        }

        let schannel = count("schannel") + count("Schannel") + count("SCHANNEL");
        let openssl = count("OpenSSL") + count("openssl");
        println!(
            "\n  VERDICT: openssl={openssl} schannel={schannel} crypt32_store_api={store_api}"
        );
        println!(
            "  {}",
            if store_api > 0 {
                "OpenSSL seeded from the Windows root store -> CurrentUser\\Root works, \
                 and Wine's crypt32 registry store is the Linux target."
            } else {
                "No Windows store API: OpenSSL trusts a PEM bundle only. The certificate \
                 must be appended to that bundle instead of imported into a store."
            }
        );

        assert!(
            schannel > 0 || openssl > 0,
            "found no TLS backend markers at all; the probe needs rethinking"
        );
        // deliberately kept: see the comment on dir
    }

    // the whole pipeline against the live steam cdn, using the smallest file in the
    // manifest so it costs a few KiB instead of 15 GiB. proves the premise: an
    // anonymous logon is enough to fetch and decrypt real chunks with a key we
    // already hold.
    //
    //     RUN_LIVE=1 cargo test -- --ignored --nocapture live_depot
    #[test]
    #[ignore = "hits the network and Steam; set RUN_LIVE=1"]
    fn live_depot_fetches_and_then_skips_the_smallest_file() {
        if std::env::var("RUN_LIVE").is_err() {
            eprintln!("skipping: set RUN_LIVE=1 to run this");
            return;
        }

        use steamroom_client::download::{CdnChunkFetcher, DepotJob, FileFilter};

        let real = std::fs::read(blob_path()).expect("run tools/fetch-depot-blob.sh");
        let (key, manifest) = split_depot_blob(&real).unwrap();
        let depot_id = manifest.depot_id.unwrap();

        let smallest = manifest
            .files
            .iter()
            .filter(|f| f.size > 0 && !f.chunks.is_empty())
            .min_by_key(|f| f.size)
            .expect("the manifest has a fetchable file");
        println!(
            "target: {} ({} bytes, {} chunk(s))",
            smallest.filename,
            smallest.size,
            smallest.chunks.len()
        );

        let dir = std::env::temp_dir().join(format!("spc-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let filter = FileFilter::from_filelist(std::slice::from_ref(&smallest.filename)).unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let stats = rt.block_on(async {
            let servers = cdn_servers().await.expect("anonymous logon + server list");
            println!("got {} content servers", servers.len());

            let fetcher = Arc::new(CdnChunkFetcher::new(
                CdnClient::new().unwrap().with_lancache(),
                CdnServerPool::new(servers),
                None,
            ));

            let job = |filter: FileFilter, key: DepotKey| {
                DepotJob::builder()
                    .depot_id(depot_id)
                    .depot_key(key)
                    .install_dir(dir.clone())
                    .verify(true)
                    .file_filter(filter)
                    .build()
                    .unwrap()
            };

            let first = job(filter, key.clone())
                .download(&manifest, fetcher.clone())
                .await
                .expect("first pass");

            // a second pass over the same directory must recognise the file
            let again =
                FileFilter::from_filelist(std::slice::from_ref(&smallest.filename)).unwrap();
            let second = job(again, key)
                .download(&manifest, fetcher)
                .await
                .expect("second pass");

            (first, second)
        });

        let (first, second) = stats;
        println!(
            "pass 1: {} written, {} skipped, {} fetched",
            first.files_completed,
            first.files_skipped,
            human(first.bytes_downloaded)
        );
        println!(
            "pass 2: {} written, {} skipped, {} fetched",
            second.files_completed,
            second.files_skipped,
            human(second.bytes_downloaded)
        );

        let landed = dir.join(smallest.normalized_path());
        assert!(landed.is_file(), "the file should exist at {landed:?}");
        assert_eq!(
            landed.metadata().unwrap().len(),
            smallest.size,
            "written file must be exactly the manifest size"
        );
        assert_eq!(first.files_completed, 1, "first pass must write the file");

        // the point of verify(true): the second pass rewrites nothing.
        // files_completed is the signal, not bytes_downloaded: the crate adds a
        // skipped file's full size to that too, so it means bytes satisfied.
        assert_eq!(second.files_completed, 0, "second pass must write nothing");
        assert!(
            second.files_skipped > first.files_skipped,
            "the target file must move from written to skipped"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // does the crate deliver events to a spawned job the way run wires it? uses the
    // real install dir and one already-present file, so it downloads nothing.
    //
    //     RUN_LIVE=1 GAME_DIR=/path cargo test --lib -- --ignored --nocapture live_events
    #[test]
    #[ignore = "needs a real install; set RUN_LIVE=1 and GAME_DIR"]
    fn live_events_reach_the_pump() {
        if std::env::var("RUN_LIVE").is_err() {
            eprintln!("skipping: set RUN_LIVE=1");
            return;
        }
        use steamroom_client::download::FileFilter;

        let dir = std::path::PathBuf::from(std::env::var("GAME_DIR").expect("GAME_DIR"));
        let real = std::fs::read(blob_path()).unwrap();
        let (key, manifest) = split_depot_blob(&real).unwrap();
        let depot_id = manifest.depot_id.unwrap();

        // a small file that already exists in the install
        let target = manifest
            .files
            .iter()
            .filter(|f| f.size > 0 && dir.join(f.normalized_path()).is_file())
            .min_by_key(|f| f.size)
            .expect("some manifest file exists in GAME_DIR");
        println!("target: {} ({} bytes)", target.filename, target.size);
        let target = target.filename.clone();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let servers = cdn_servers().await.unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let job = DepotJob::builder()
                .depot_id(depot_id)
                .depot_key(key)
                .install_dir(dir.clone())
                .verify(true)
                .file_filter(FileFilter::from_filelist(std::slice::from_ref(&target)).unwrap())
                .event_sender(tx)
                .build()
                .unwrap();
            let fetcher = Arc::new(CdnChunkFetcher::new(
                CdnClient::new().unwrap().with_lancache(),
                CdnServerPool::new(servers),
                None,
            ));
            let mut download = tokio::spawn(async move { job.download(&manifest, fetcher).await });

            let mut seen = 0usize;
            loop {
                tokio::select! {
                    biased;
                    Some(ev) = rx.recv() => { seen += 1; println!("  event {seen}: {ev:?}"); }
                    done = &mut download => { done.unwrap().unwrap(); break; }
                }
            }
            // drain whatever landed after the job returned
            while let Ok(ev) = rx.try_recv() {
                seen += 1;
                println!("  late event {seen}: {ev:?}");
            }
            println!("total events: {seen}");
            assert!(seen > 0, "the crate delivered no events at all");
        });
    }
}
