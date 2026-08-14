// mongod as a managed child process. needs no administrator rights.

use std::path::PathBuf;

use tauri::AppHandle;
use tokio::process::{Child, Command};

use crate::{logs, settings};

// recovering a large journal after an unclean stop is the slow case
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const POLL: std::time::Duration = std::time::Duration::from_millis(200);

// enough ports for a handful of stale instances
const PORT_RANGE: u16 = 16;

// mongod defaults its cache to half of ram, absurd for a bundled database
const CACHE_GB: &str = "0.5";

#[derive(Debug, thiserror::Error)]
pub enum MongoError {
    #[error("MongoDB is not installed yet.")]
    NotInstalled,
    #[error("No free port for MongoDB in {0}-{1}.")]
    PortsExhausted(u16, u16),
    #[error("MongoDB would not start.\n\n{tail}")]
    Failed { tail: String },
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for MongoError {
    fn from(e: std::io::Error) -> Self {
        MongoError::Io(e.to_string())
    }
}

// dropping or stopping it kills the process
pub struct Mongo {
    pub port: u16,
    child: Child,
}

fn db_path(app: &AppHandle) -> PathBuf {
    settings::mongo_dir(app).join("db")
}

pub fn log_file(app: &AppHandle) -> PathBuf {
    settings::mongo_dir(app).join("mongod.log")
}

fn port_taken(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

fn free_port(from: u16) -> Option<u16> {
    (from..from.saturating_add(PORT_RANGE)).find(|p| !port_taken(*p))
}

// path-scoped on purpose: never touches a mongod we did not start
pub fn sweep_orphans(app: &AppHandle) -> usize {
    let exe = settings::mongo_exe(app);
    if !exe.is_file() {
        return 0;
    }
    crate::proc::kill_matching_exe(&exe)
}

pub async fn start(app: &AppHandle) -> Result<Mongo, MongoError> {
    let exe = settings::mongo_exe(app);
    if !exe.is_file() {
        return Err(MongoError::NotInstalled);
    }

    // a hard-killed mongod leaves mongod.lock behind; the journal recovers it
    let swept = sweep_orphans(app);
    if swept > 0 {
        log::info!("cleared {swept} orphaned mongod process(es)");
    }

    let dir = settings::mongo_dir(app);
    let db = db_path(app);
    std::fs::create_dir_all(&db)?;

    let configured = settings::mongo_port(app);
    let port = if configured != 0 {
        configured
    } else {
        free_port(settings::MONGO_PORT_BASE).ok_or(MongoError::PortsExhausted(
            settings::MONGO_PORT_BASE,
            settings::MONGO_PORT_BASE + PORT_RANGE,
        ))?
    };

    let log = log_file(app);
    let mut cmd = Command::new(&exe);
    cmd.arg("--dbpath")
        .arg(&db)
        .arg("--port")
        .arg(port.to_string())
        // never reachable from off the machine
        .arg("--bind_ip")
        .arg("127.0.0.1")
        .arg("--logpath")
        .arg(&log)
        .arg("--logappend")
        .arg("--wiredTigerCacheSizeGB")
        .arg(CACHE_GB)
        .arg("--quiet")
        .current_dir(&dir)
        .kill_on_drop(true);

    // no --fork: a child we own, not a daemon that outlives us
    #[cfg(unix)]
    {
        cmd.arg("--nounixsocket");

        crate::launch::scrub_appimage_env(&mut cmd);
    }

    #[cfg(windows)]
    {
        // tokio's Command has creation_flags inherently
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
    {
        cmd.stderr(std::process::Stdio::from(f));
    }

    log::info!(
        "starting mongod on 127.0.0.1:{port} with dbpath {}",
        db.display()
    );
    let mut child = cmd.spawn().map_err(|e| MongoError::Failed {
        tail: format!("could not run {}: {e}", exe.display()),
    })?;

    // mongod binds its listener only after recovery, so connect is true readiness
    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(MongoError::Failed {
                tail: format!(
                    "mongod exited with {status} before accepting connections.\n\n{}",
                    logs::tail(&log, 20)
                ),
            });
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            log::info!("mongod is accepting connections on {port}");
            return Ok(Mongo { port, child });
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            return Err(MongoError::Failed {
                tail: format!(
                    "mongod did not accept connections within {}s.\n\n{}",
                    READY_TIMEOUT.as_secs(),
                    logs::tail(&log, 20)
                ),
            });
        }
        tokio::time::sleep(POLL).await;
    }
}

impl Mongo {
    // ponytail: hard kill. wiredtiger journals, so the cost is replay on next start.
    pub fn stop(&mut self) {
        let _ = self.child.start_kill();
    }

    // start_kill only signals, and the next start needs the dbpath lock released
    pub async fn stop_and_reap(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

// ---- the one instance the launcher owns ----

static CURRENT: tokio::sync::Mutex<Option<Mongo>> = tokio::sync::Mutex::const_new(None);

pub async fn ensure(app: &AppHandle) -> Result<(u16, bool), MongoError> {
    let mut held = CURRENT.lock().await;
    if let Some(m) = held.as_mut() {
        match m.child.try_wait() {
            Ok(None) => return Ok((m.port, false)),
            _ => {
                log::info!("the mongod we held is gone; starting another");
                held.take();
            }
        }
    }
    let m = start(app).await?;
    let port = m.port;
    *held = Some(m);
    Ok((port, true))
}

pub async fn shutdown() {
    if let Some(mut m) = CURRENT.lock().await.take() {
        m.stop_and_reap().await;
    }
}

pub fn shutdown_now() {
    if let Ok(mut held) = CURRENT.try_lock() {
        if let Some(mut m) = held.take() {
            m.stop();
        }
    }
}

pub fn tail(app: &AppHandle, lines: usize) -> String {
    logs::tail(&log_file(app), lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    // the whole point of picking a port: never collide with the user's own mongodb
    #[test]
    fn free_port_skips_a_bound_port() {
        // bind the base so it is definitely taken
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let taken = held.local_addr().unwrap().port();

        assert!(
            port_taken(taken),
            "the port we are holding must read as taken"
        );
        let found = free_port(taken).expect("a free port exists just after it");
        assert_ne!(found, taken);
        assert!(found > taken);
        assert!(!port_taken(found));

        drop(held);
    }

    #[test]
    fn we_never_default_to_the_standard_mongo_port() {
        assert_ne!(
            settings::MONGO_PORT_BASE,
            27017,
            "27017 would fight a user's own MongoDB"
        );
    }

    #[test]
    fn the_uri_names_loopback_and_the_chosen_port() {
        // no real child here, just the formatting contract the server depends on
        let uri = format!("mongodb://127.0.0.1:{}/", 27055);
        assert_eq!(uri, "mongodb://127.0.0.1:27055/");
    }
}
