// finding and stopping processes we own.
//
// ownership is proven by executable path, never by image name, so an unrelated
// copy of the game in a steam library is never touched.
//
// enumerating costs ~55 ms, so the repeating callers avoid it: a running game
// is watched by its own pid, and the idle poll caches its answer.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

// how long the idle poll reuses its last answer. only affects noticing a game
// we did not start, which can wait.
const ADOPT_TTL: Duration = Duration::from_secs(10);

fn refresh_kind() -> ProcessRefreshKind {
    // the exe path is the only field any of this needs
    ProcessRefreshKind::new().with_exe(UpdateKind::Always)
}

fn snapshot() -> System {
    System::new_with_specifics(RefreshKind::new().with_processes(refresh_kind()))
}

// canonicalised, so a symlinked or relative path still compares equal
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn is_game_exe(exe: &Path) -> bool {
    exe.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_lowercase().starts_with("prospect-win64"))
        .unwrap_or(false)
}

// the ownership predicate, and the only place it is decided: an exe is ours iff
// it resolves inside a directory we installed. both sides canonicalised first.
fn owned_by_root(exe: &Path, root: &Path) -> bool {
    real(exe).starts_with(real(root))
}

// returns how many were signalled
pub fn kill_matching_exe(exe: &Path) -> usize {
    let want = real(exe);
    let sys = snapshot();
    sys.processes()
        .iter()
        .filter(|(_, p)| p.exe().map(|e| real(e) == want).unwrap_or(false))
        .filter(|(_, p)| p.kill())
        .count()
}

// newest first, so a supervisor cannot restart the child we just stopped
pub fn kill_under(root: &Path) -> usize {
    let sys = snapshot();
    let mut found: Vec<_> = sys
        .processes()
        .iter()
        .filter(|(_, p)| p.exe().map(|e| owned_by_root(e, root)).unwrap_or(false))
        .map(|(pid, p)| (*pid, p.start_time()))
        .collect();
    found.sort_by_key(|(_, started)| std::cmp::Reverse(*started));

    let killed = found
        .iter()
        .filter_map(|(pid, _)| sys.process(*pid))
        .filter(|p| p.kill())
        .count();
    if killed > 0 {
        // whatever we just killed, the cached answer is now wrong
        invalidate();
    }
    killed
}

// filters on the exe name before canonicalising, so no match costs no syscalls
pub fn find_game(game_dir: &Path) -> Option<Pid> {
    let sys = snapshot();
    sys.processes()
        .iter()
        // name first: a string compare on data we already have
        .filter(|(_, p)| p.exe().map(is_game_exe).unwrap_or(false))
        .find(|(_, p)| p.exe().map(|e| owned_by_root(e, game_dir)).unwrap_or(false))
        .map(|(pid, _)| *pid)
}

// watches one known pid without re-enumerating: one /proc entry, not every one
pub struct Watch {
    pid: Pid,
    sys: System,
}

impl Watch {
    pub fn new(pid: Pid) -> Self {
        Watch {
            pid,
            sys: System::new(),
        }
    }

    // true while the process is still there
    pub fn alive(&mut self) -> bool {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            // prune it the moment it dies, so process() goes None
            true,
            refresh_kind(),
        );
        self.sys.process(self.pid).is_some()
    }
}

fn is_steam(p: &sysinfo::Process) -> bool {
    let name = p.name().to_string_lossy().to_lowercase();
    name == "steam" || name == "steam.exe"
}

// both answers come from one scan, so steam's state costs nothing extra
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    pub game: bool,
    pub steam: bool,
}

// the cached form, for the idle poll
static OBSERVED: Mutex<Option<(Instant, PathBuf, Observed)>> = Mutex::new(None);

// called wherever we knowingly change what is running
pub fn invalidate() {
    *OBSERVED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// one scan answering both questions
pub fn observe_cached(game_dir: &Path) -> Observed {
    let mut guard = OBSERVED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, dir, answer)) = guard.as_ref() {
        // a changed game directory invalidates the answer as surely as time does
        if at.elapsed() < ADOPT_TTL && dir == game_dir {
            return *answer;
        }
    }

    let sys = snapshot();
    let answer = Observed {
        game: sys
            .processes()
            .values()
            .filter(|p| p.exe().map(is_game_exe).unwrap_or(false))
            .any(|p| p.exe().map(|e| owned_by_root(e, game_dir)).unwrap_or(false)),
        steam: sys.processes().values().any(is_steam),
    };
    *guard = Some((Instant::now(), game_dir.to_path_buf(), answer));
    answer
}

// matched by name because we do not own it. enumerates; prefer observe_cached.
pub fn steam_is_running() -> bool {
    // names are populated without the exe refresh, so this is the cheaper kind
    let sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.processes().values().any(is_steam)
}

#[cfg(test)]
mod tests {
    use super::*;

    // the core safety property: an executable outside our install is never ours
    #[test]
    fn an_exe_outside_our_root_is_never_owned() {
        let me = std::env::current_exe().unwrap();
        let nowhere = std::env::temp_dir().join("spc-definitely-not-an-install-x9");

        assert!(!owned_by_root(&me, &nowhere));
        assert!(!owned_by_root(Path::new("/usr/bin/steam"), &nowhere));

        // compares components, not text: "/games/spcycle2" must not match "/games/spcycle"
        assert!(!owned_by_root(
            Path::new("/games/spcycle2/game.exe"),
            Path::new("/games/spcycle")
        ));
    }

    // a root that does contain the exe must match, or teardown stops nothing
    #[test]
    fn an_exe_inside_our_root_is_owned() {
        let me = std::env::current_exe().unwrap();
        assert!(owned_by_root(&me, me.parent().unwrap()));
        assert!(owned_by_root(&me, me.parent().unwrap().parent().unwrap()));
    }

    #[test]
    fn game_detection_only_matches_the_shipping_exe() {
        // the test binary is not named Prospect-Win64-*, so this must be false
        let me = std::env::current_exe().unwrap();
        assert!(find_game(me.parent().unwrap()).is_none());
    }

    #[test]
    fn is_game_exe_matches_the_shipping_binary_only() {
        assert!(is_game_exe(Path::new("/x/Prospect-Win64-Shipping.exe")));
        assert!(is_game_exe(Path::new("/x/prospect-win64-shipping.exe")));
        assert!(!is_game_exe(Path::new("/x/Prospect.Client.Loader.exe")));
        assert!(!is_game_exe(Path::new("/x/Prospect_BE.exe")));
        assert!(!is_game_exe(Path::new("/x/mongod")));
    }

    // otherwise a play session either never ends or ends immediately
    #[test]
    fn watch_tracks_a_real_process_to_its_death() {
        let child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "/bin/sleep" })
            .arg(if cfg!(windows) { "/c" } else { "30" })
            .spawn()
            .expect("spawn a child to watch");
        let pid = Pid::from_u32(child.id());

        let mut watch = Watch::new(pid);
        assert!(watch.alive(), "a running child must read as alive");

        let mut child = child;
        child.kill().ok();
        child.wait().ok();

        // the pid is reaped; give the OS a moment on slower machines
        let mut dead = false;
        for _ in 0..50 {
            if !watch.alive() {
                dead = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(dead, "a reaped child must stop reading as alive");
    }

    // or changing the install path would report the old one's game as running
    #[test]
    fn the_cache_is_keyed_on_the_directory() {
        invalidate();
        let a = std::env::temp_dir().join("spc-cache-a");
        let b = std::env::temp_dir().join("spc-cache-b");

        assert!(!observe_cached(&a).game);
        assert!(!observe_cached(&a).game, "second call is served from cache");
        assert!(!observe_cached(&b).game, "a different dir is not cached");

        invalidate();
        assert!(OBSERVED.lock().unwrap().is_none());
    }

    // otherwise the lamp and the launch gate could disagree
    #[test]
    fn one_scan_answers_game_and_steam_together() {
        invalidate();
        let dir = std::env::temp_dir().join("spc-observe");
        let seen = observe_cached(&dir);
        assert!(!seen.game, "no game runs from a temp directory");
        assert_eq!(
            seen.steam,
            steam_is_running(),
            "the folded scan must agree with the dedicated check"
        );
        invalidate();
    }
}

#[cfg(test)]
mod cost {
    // what the repeating callers actually cost:
    //     cargo test --lib -- --ignored --nocapture snapshot_cost
    #[test]
    #[ignore = "timing measurement; run with --nocapture"]
    fn snapshot_cost() {
        let dir = std::env::temp_dir();
        let n = 20;

        let _ = super::find_game(&dir);
        let start = std::time::Instant::now();
        for _ in 0..n {
            let _ = super::find_game(&dir);
        }
        let full = start.elapsed() / n;

        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut watch = super::Watch::new(sysinfo::Pid::from_u32(child.id()));
        let _ = watch.alive();
        let start = std::time::Instant::now();
        for _ in 0..n {
            let _ = watch.alive();
        }
        let watched = start.elapsed() / n;
        let mut child = child;
        child.kill().ok();
        child.wait().ok();

        let procs = super::snapshot().processes().len();
        println!("{procs} processes");
        println!("  full scan (once per launch):  {full:?}");
        println!("  Watch::alive (every 2s):      {watched:?}");
        println!(
            "  session cost: {:.4}% of a core, was {:.3}%",
            watched.as_secs_f64() / 2.0 * 100.0,
            full.as_secs_f64() / 2.0 * 100.0
        );
        assert!(
            watched < full / 5,
            "watching one pid should be far cheaper than a full scan"
        );
    }
}
