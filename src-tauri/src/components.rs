// fetching the three pieces the game needs that are not in the steam depot:
// the local server, the client loader, and mongod.
//
// each is a zip named in a manifest published alongside our own releases.
// nothing is trusted on the way in: every archive is hashed as it streams, and
// nothing is moved into a live directory until every archive has passed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tauri::AppHandle;

use crate::{progress, settings, show_bar};

// overridable so a candidate manifest can be tried without shipping a build.
// one manifest per platform, because mongod is a different binary on each and a
// single mongod key can only carry one url.
const MANIFEST_URL: &str = if cfg!(windows) {
    "https://github.com/itsOwen/spcycle-launcher/releases/latest/download/components-windows.json"
} else {
    "https://github.com/itsOwen/spcycle-launcher/releases/latest/download/components-linux.json"
};
const MANIFEST_URL_ENV: &str = "SPCYCLE_COMPONENTS_URL";

// the cap stops a wrong or hostile url filling the disk before the hash rejects it
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

const STAMP: &str = ".spc_components";

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("Could not reach the component server.")]
    Offline,
    #[error("{name} did not match its checksum and was discarded.")]
    BadDigest { name: String },
    #[error("{0}")]
    Message(String),
}

impl From<std::io::Error> for ComponentError {
    fn from(e: std::io::Error) -> Self {
        ComponentError::Message(e.to_string())
    }
}

// where an archive's contents belong once verified
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    // the local server, its config, and generate_ssl.exe
    ServerDir,
    // dropped in beside the game's own binaries
    Win64,
    // bin/mongod[.exe] and anything it links
    MongoDir,
}

impl Target {
    fn resolve(self, app: &AppHandle) -> PathBuf {
        match self {
            Target::ServerDir => settings::server_dir(app),
            Target::Win64 => settings::win64_dir(app),
            Target::MongoDir => settings::mongo_dir(app),
        }
    }
}

#[derive(Debug)]
pub struct Component {
    pub name: &'static str,
    pub target: Target,
    // must exist afterwards for the component to count as present
    pub proof: &'static str,
}

// the only names the launcher will ever stage. the plan is driven by this list,
// never by the manifest's own keys, so a manifest cannot name a component
// ../../etc/cron.d/pwn nor add files we never asked for.
pub const COMPONENTS: &[Component] = &[
    Component {
        name: "server",
        target: Target::ServerDir,
        proof: "Prospect.Server.Api.exe",
    },
    Component {
        name: "loaderpack",
        target: Target::Win64,
        proof: "Prospect.Client.Loader.exe",
    },
    Component {
        name: "mongod",
        target: Target::MongoDir,
        proof: if cfg!(windows) {
            "bin/mongod.exe"
        } else {
            "bin/mongod"
        },
    },
];

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub files: HashMap<String, Entry>,
}

#[derive(Debug, Deserialize)]
pub struct Entry {
    pub url: String,
    // lowercase hex sha-256 of the archive's bytes
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
    // a leading path inside the archive to drop
    #[serde(default)]
    pub strip_prefix: Option<String>,
}

fn manifest_url() -> String {
    std::env::var(MANIFEST_URL_ENV).unwrap_or_else(|_| MANIFEST_URL.to_string())
}

fn client() -> Result<reqwest::Client, ComponentError> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    Ok(CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(8))
                .user_agent(concat!("spcycle-launcher/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("a rustls client always builds")
        })
        .clone())
}

fn stamp_path(app: &AppHandle) -> PathBuf {
    settings::components_dir(app).join(STAMP)
}

pub fn staged_version(app: &AppHandle) -> Option<String> {
    std::fs::read_to_string(stamp_path(app))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// cheap enough for the phase poll: three stat calls
pub fn complete(app: &AppHandle) -> bool {
    COMPONENTS.iter().all(|c| {
        let mut p = c.target.resolve(app);
        for part in c.proof.split('/') {
            p = p.join(part);
        }
        p.is_file()
    })
}

pub async fn manifest() -> Result<Manifest, ComponentError> {
    let url = manifest_url();
    if !url.starts_with("https://") {
        return Err(ComponentError::Message(
            "The component manifest URL is not https; refusing to fetch executables over it."
                .into(),
        ));
    }
    client()?
        .get(url)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| ComponentError::Offline)?
        .error_for_status()
        .map_err(|e| ComponentError::Message(format!("The component server returned {e}.")))?
        .json()
        .await
        .map_err(|e| {
            ComponentError::Message(format!("The component manifest could not be read: {e}"))
        })
}

// checked before a byte is requested: an entry we could never verify is not
// worth fetching
fn validate_digest(entry: &Entry, name: &str) -> Result<String, ComponentError> {
    let expected = entry.sha256.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ComponentError::Message(format!(
            "The manifest gives `{name}` a sha256 that is not 64 hex characters."
        )));
    }
    Ok(expected)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// driven by COMPONENTS, so the manifest cannot widen the set or escape the target
fn plan(want: &Manifest) -> Result<Vec<(&'static Component, &Entry)>, ComponentError> {
    let mut out = Vec::with_capacity(COMPONENTS.len());
    let mut missing = Vec::new();
    for c in COMPONENTS {
        match want.files.get(c.name) {
            Some(e) => out.push((c, e)),
            None => missing.push(c.name),
        }
    }
    if !missing.is_empty() {
        return Err(ComponentError::Message(format!(
            "The component manifest is missing {}.",
            missing.join(", ")
        )));
    }
    Ok(out)
}

// refuses the archive unless the bytes hash to what the manifest promised
async fn fetch_one(
    app: &AppHandle,
    name: &str,
    entry: &Entry,
    dest: &Path,
) -> Result<(), ComponentError> {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    let expected = validate_digest(entry, name)?;
    if !entry.url.starts_with("https://") {
        return Err(ComponentError::Message(format!(
            "`{name}` is not served over https; refusing it."
        )));
    }

    let res = client()?
        .get(&entry.url)
        .send()
        .await
        .map_err(|_| ComponentError::Offline)?
        .error_for_status()
        .map_err(|e| ComponentError::Message(format!("The server returned {e} for {name}.")))?;

    let declared = entry.size.or_else(|| res.content_length()).unwrap_or(0);
    if declared > MAX_FILE_BYTES {
        return Err(ComponentError::Message(format!(
            "{name} claims to be {declared} bytes, far larger than expected; refusing it."
        )));
    }

    let mut file = tokio::fs::File::create(dest).await?;
    let mut hasher = Sha256::new();
    let mut stream = res.bytes_stream();
    let mut done = 0u64;
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| ComponentError::Message(format!("Download interrupted: {e}")))?;
        done += chunk.len() as u64;
        if done > MAX_FILE_BYTES {
            return Err(ComponentError::Message(format!(
                "{name} did not stop at a sensible size."
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        if last.elapsed().as_millis() >= 100 {
            progress(app, done, declared, &format!("Downloading {name}"));
            last = std::time::Instant::now();
        }
    }
    file.flush().await?;
    drop(file);

    if hex(&hasher.finalize()) != expected {
        // leave nothing half-verified for a later run to mistake for good
        let _ = std::fs::remove_file(dest);
        return Err(ComponentError::BadDigest {
            name: name.to_string(),
        });
    }
    if let Some(size) = entry.size {
        if size != done {
            let _ = std::fs::remove_file(dest);
            return Err(ComponentError::Message(format!(
                "{name} is {done} bytes but the manifest says {size}."
            )));
        }
    }
    Ok(())
}

// refuses any entry that would land outside into
pub fn extract(
    archive: &Path,
    into: &Path,
    strip_prefix: Option<&str>,
) -> Result<(), ComponentError> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| ComponentError::Message(format!("Not a readable zip: {e}")))?;

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| ComponentError::Message(format!("Unreadable zip entry: {e}")))?;

        // enclosed_name rejects absolute paths and .. segments outright
        let Some(rel) = entry.enclosed_name() else {
            return Err(ComponentError::Message(format!(
                "The archive contains an unsafe path: {}",
                entry.name()
            )));
        };

        let rel = match strip_prefix {
            Some(prefix) => match rel.strip_prefix(prefix) {
                Ok(stripped) => stripped.to_path_buf(),
                // entries outside the declared prefix are not ours to place
                Err(_) => continue,
            },
            None => rel,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }

        let out = into.join(&rel);
        // belt and braces: enclosed_name guarantees this, but the join is what
        // actually has to be safe
        if !out.starts_with(into) {
            return Err(ComponentError::Message(format!(
                "The archive tried to write outside its target: {}",
                rel.display()
            )));
        }

        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // replace rather than append: a stale file must not survive with old contents
        let _ = std::fs::remove_file(&out);
        let mut sink = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut sink)?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            // mask down: an archive should not be able to hand out setuid
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755));
        }
    }
    Ok(())
}

// a scratch directory that removes itself, so a failed run leaves nothing
struct CleanUp(PathBuf);

impl Drop for CleanUp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn workdir(app: &AppHandle) -> Result<PathBuf, ComponentError> {
    let dir = settings::components_dir(app).join(".staging");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

// Ok(false) means the staged copy already matches the manifest version
pub async fn ensure(app: &AppHandle) -> Result<bool, ComponentError> {
    let want = manifest().await?;
    let wanted = plan(&want)?;

    if complete(app) && staged_version(app).as_deref() == Some(want.version.as_str()) {
        return Ok(false);
    }

    let work = workdir(app)?;
    let _guard = CleanUp(work.clone());

    show_bar(app, true);
    let result = stage_all(app, &wanted, &work).await;
    show_bar(app, false);
    result?;

    // the stamp goes down last: a partial run must never look like a good one
    std::fs::write(stamp_path(app), &want.version)?;
    Ok(true)
}

async fn stage_all(
    app: &AppHandle,
    wanted: &[(&'static Component, &Entry)],
    work: &Path,
) -> Result<(), ComponentError> {
    // download and verify everything first...
    for (c, entry) in wanted {
        let archive = work.join(format!("{}.zip", c.name));
        fetch_one(app, c.name, entry, &archive).await?;
    }

    // ...then extract, so nothing reaches a live directory until every archive
    // has proven itself. off the runtime: this is tens of MB of synchronous
    // inflate, and on a worker it stalls the progress pump and the state poll.
    for (c, entry) in wanted {
        let archive = work.join(format!("{}.zip", c.name));
        let target = c.target.resolve(app);
        progress(app, 0, 0, &format!("Installing {}", c.name));
        let strip = entry.strip_prefix.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&target)?;
            extract(&archive, &target, strip.as_deref())
        })
        .await
        .map_err(|e| ComponentError::Message(e.to_string()))??;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(sha: &str) -> Entry {
        Entry {
            url: "https://example.com/x.zip".into(),
            sha256: sha.into(),
            size: None,
            strip_prefix: None,
        }
    }

    // the manifest is data from the network. it must not widen the set of things
    // the launcher writes, nor name a path that escapes.
    #[test]
    fn only_known_component_names_are_ever_staged() {
        let mut files = HashMap::new();
        files.insert("../../etc/cron.d/pwn".to_string(), entry(&"a".repeat(64)));
        files.insert("totally_new_thing".to_string(), entry(&"b".repeat(64)));
        let m = Manifest {
            version: "1".into(),
            files,
        };

        // the plan cannot be satisfied, because none of our names are there
        let err = plan(&m).expect_err("a manifest without our components must be refused");
        let msg = err.to_string();
        for c in COMPONENTS {
            assert!(
                msg.contains(c.name),
                "{msg} should name the missing {}",
                c.name
            );
        }
        assert!(
            !msg.contains("cron.d"),
            "the manifest's own keys must never drive the plan"
        );
    }

    #[test]
    fn a_digest_that_is_not_a_sha256_is_refused_up_front() {
        assert!(
            validate_digest(&entry("deadbeef"), "server").is_err(),
            "too short"
        );
        assert!(
            validate_digest(&entry(&"z".repeat(64)), "server").is_err(),
            "not hex"
        );
        let good = "a".repeat(64);
        assert_eq!(validate_digest(&entry(&good), "server").unwrap(), good);
        // case and surrounding space are normalised, not rejected
        let mixed = format!("  {}  ", "AB".repeat(32));
        assert_eq!(
            validate_digest(&entry(&mixed), "server").unwrap(),
            "ab".repeat(32)
        );
    }

    #[test]
    fn sha256_of_abc_matches_the_published_vector() {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            hex(&h.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // a zip entry naming ../ must not write outside the target
    #[test]
    fn extraction_refuses_to_escape_its_target() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let dir = std::env::temp_dir().join(format!("spc-zip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("evil.zip");
        let target = dir.join("target");
        std::fs::create_dir_all(&target).unwrap();

        {
            let f = std::fs::File::create(&archive).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("../escaped.txt", SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"pwned").unwrap();
            z.start_file("fine.txt", SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"ok").unwrap();
            z.finish().unwrap();
        }

        // the whole archive is refused, not just the bad entry
        let err = extract(&archive, &target, None)
            .expect_err("an archive containing ../ must be refused outright");
        assert!(
            err.to_string().contains("unsafe path"),
            "the error should say why: {err}"
        );
        assert!(
            !dir.join("escaped.txt").exists(),
            "an entry escaped the target directory"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn strip_prefix_drops_a_wrapping_directory() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let dir = std::env::temp_dir().join(format!("spc-zip2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("wrapped.zip");
        let target = dir.join("target");

        {
            let f = std::fs::File::create(&archive).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("mongodb-8.2.1/bin/mongod", SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"binary").unwrap();
            z.finish().unwrap();
        }

        extract(&archive, &target, Some("mongodb-8.2.1")).unwrap();
        assert_eq!(std::fs::read(target.join("bin/mongod")).unwrap(), b"binary");

        std::fs::remove_dir_all(&dir).ok();
    }

    // catches a hand-edited digest or a copy-pasted url before it reaches a release
    #[test]
    fn the_published_manifests_parse_and_validate() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools");
        for plat in ["windows", "linux"] {
            let path = dir.join(format!("components-{plat}.json"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{} is missing: {e}", path.display()));
            let manifest: Manifest = serde_json::from_str(&text).expect("it parses as a Manifest");

            // the same check plan() makes, so a manifest that would be rejected at
            // install time fails here instead
            plan(&manifest).unwrap_or_else(|e| panic!("{plat}: {e}"));

            for c in COMPONENTS {
                let e = &manifest.files[c.name];
                validate_digest(e, c.name).expect("the digest is 64 hex characters");
                assert!(
                    e.url.starts_with("https://"),
                    "{plat}/{} must be served over https",
                    c.name
                );
                assert!(
                    e.size.is_some(),
                    "{plat}/{} should declare its size",
                    c.name
                );
            }

            assert!(
                manifest.files["mongod"].url.contains(plat),
                "{plat} must name its own mongod build"
            );
        }
    }

    // the loader archive wraps everything in LoaderPack/, so extraction needs it
    #[test]
    fn the_loader_entry_strips_its_wrapping_directory() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/components-linux.json");
        let manifest: Manifest =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            manifest.files["loaderpack"].strip_prefix.as_deref(),
            Some("LoaderPack")
        );
    }

    // does every entry in the published manifests actually extract into the layout
    // proof checks for? downloads the real archives; mongod comes from artifacts/,
    // since its url only resolves once a release is published.
    //
    //     RUN_LIVE=1 cargo test --lib -- --ignored --nocapture published_components
    #[test]
    #[ignore = "downloads ~65 MB; set RUN_LIVE=1"]
    fn live_published_components_extract_to_their_proof_files() {
        if std::env::var("RUN_LIVE").is_err() {
            eprintln!("skipping: set RUN_LIVE=1 to run this");
            return;
        }
        use sha2::{Digest, Sha256};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let plat = if cfg!(windows) { "windows" } else { "linux" };
        let text = std::fs::read_to_string(root.join(format!("tools/components-{plat}.json")))
            .expect("the manifest for this platform");
        let manifest: Manifest = serde_json::from_str(&text).unwrap();

        let work = std::env::temp_dir().join(format!("spc-components-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();

        for c in COMPONENTS {
            let entry = &manifest.files[c.name];
            let expected = validate_digest(entry, c.name).unwrap();

            let bytes = if c.name == "mongod" {
                let local = root.join(format!("artifacts/mongod-{plat}.zip"));
                std::fs::read(&local).unwrap_or_else(|e| {
                    panic!("{}: {e} — run tools/repack-mongod.sh", local.display())
                })
            } else {
                let out = std::process::Command::new("curl")
                    .args(["-sL", &entry.url])
                    .output()
                    .expect("curl");
                assert!(out.status.success(), "curl failed for {}", c.name);
                out.stdout
            };

            let got = hex(&Sha256::digest(&bytes));
            assert_eq!(
                got, expected,
                "{} does not match its manifest digest",
                c.name
            );

            let archive = work.join(format!("{}.zip", c.name));
            std::fs::write(&archive, &bytes).unwrap();
            let target = work.join(c.name);
            extract(&archive, &target, entry.strip_prefix.as_deref())
                .unwrap_or_else(|e| panic!("{} failed to extract: {e}", c.name));

            let mut proof = target.clone();
            for part in c.proof.split('/') {
                proof = proof.join(part);
            }
            assert!(
                proof.is_file(),
                "{} extracted without its proof file {}",
                c.name,
                c.proof
            );
            println!(
                "  {:<11} {:>9} -> {}",
                c.name,
                format!("{:.1} MiB", bytes.len() as f64 / 1048576.0),
                c.proof
            );
        }

        std::fs::remove_dir_all(&work).ok();
    }
}
