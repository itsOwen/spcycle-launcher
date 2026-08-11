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
    // exe for native processes, cmd for the ones wine hosts: their exe is always
    // proton's preloader, and the windows path is in the command line
    ProcessRefreshKind::new()
        .with_exe(UpdateKind::Always)
        .with_cmd(UpdateKind::Always)
        // cwd is not free, but the loader starts the game by bare name and that is
        // the only thing that resolves it. see wine_arg_to_host.
        .with_cwd(UpdateKind::Always)
}

fn snapshot() -> System {
    System::new_with_specifics(RefreshKind::new().with_processes(refresh_kind()))
}

// canonicalised, so a symlinked or relative path still compares equal
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

// wine reports proton's preloader as the executable of every windows process it
// hosts, so /proc/<pid>/exe never points into the install. the .exe path is in
// the command line instead, as a windows path.
//
// wine copies a child's command line verbatim from whatever the parent passed to
// CreateProcess, so it is only sometimes absolute. the three shapes that turn up:
//
//   Z:\home\...\Prospect-Win64-Shipping.exe   the host filesystem
//   C:\windows\system32\...                   inside the prefix, never ours
//   Prospect-Win64-Shipping.exe               relative to the process's cwd
//
// the last one is what the loader uses to start the game, and resolving it is
// the only reason the scan pays for cwd.
#[cfg(unix)]
fn wine_arg_to_host(arg: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    // a native process is judged by its own exe path, so only windows binaries
    // are worth resolving here
    if !arg.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    let win = arg.replace('\\', "/");

    let mut c = win.chars();
    let drive = match (c.next(), c.next()) {
        (Some(d), Some(':')) if d.is_ascii_alphabetic() => Some(d),
        _ => None,
    };
    if let Some(d) = drive {
        // Z: is wine's view of the host root; any other drive lives inside the
        // prefix and is not something we started from the install
        if !d.eq_ignore_ascii_case(&'z') {
            return None;
        }
        let rest = win.get(2..)?.strip_prefix('/')?;
        return Some(PathBuf::from(format!("/{rest}")));
    }

    // bare or relative: only the process's working directory resolves it, and it
    // has to actually be there, or an unrelated process sitting in the install
    // directory could pass for the game
    let joined = cwd?.join(&win);
    joined.is_file().then_some(joined)
}

#[cfg(unix)]
fn hosted_exe(p: &sysinfo::Process) -> Option<PathBuf> {
    wine_arg_to_host(p.cmd().first()?.to_str()?, p.cwd())
}

#[cfg(not(unix))]
fn hosted_exe(_p: &sysinfo::Process) -> Option<PathBuf> {
    None
}

// the path a process is judged by: the windows exe when wine is hosting one,
// otherwise the real executable
fn exe_of(p: &sysinfo::Process) -> Option<PathBuf> {
    hosted_exe(p).or_else(|| p.exe().map(|e| e.to_path_buf()))
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
        .filter(|(_, p)| exe_of(p).map(|e| real(&e) == want).unwrap_or(false))
        .filter(|(_, p)| p.kill())
        .count()
}

// newest first, so a supervisor cannot restart the child we just stopped
pub fn kill_under(root: &Path) -> usize {
    let sys = snapshot();
    let mut found: Vec<_> = sys
        .processes()
        .iter()
        .filter(|(_, p)| exe_of(p).map(|e| owned_by_root(&e, root)).unwrap_or(false))
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
    let found = sys
        .processes()
        .iter()
        // name first: a string compare on data we already have
        .filter(|(_, p)| exe_of(p).map(|e| is_game_exe(&e)).unwrap_or(false))
        .find(|(_, p)| {
            exe_of(p)
                .map(|e| owned_by_root(&e, game_dir))
                .unwrap_or(false)
        });
    // says which process was adopted, or names the ones that looked like the game
    // but resolved outside the install
    match found {
        Some((pid, p)) => {
            log::info!(
                "the game is pid {pid}, running {}",
                exe_of(p).unwrap_or_default().display()
            );
            Some(*pid)
        }
        None => {
            for p in sys.processes().values() {
                if let Some(e) = exe_of(p) {
                    if is_game_exe(&e) {
                        log::warn!("{} is not inside {}", e.display(), game_dir.display());
                    }
                }
            }
            None
        }
    }
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
            .filter(|p| exe_of(p).map(|e| is_game_exe(&e)).unwrap_or(false))
            .any(|p| {
                exe_of(p)
                    .map(|e| owned_by_root(&e, game_dir))
                    .unwrap_or(false)
            }),
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

    // ownership is read off cmd, and a bare exe name is only resolvable via cwd.
    // sysinfo populates neither unless asked, and forgetting one means the game is
    // never found: the launcher then sits in "starting" until it times out.
    #[test]
    fn the_scan_asks_for_the_fields_ownership_depends_on() {
        let kind = refresh_kind();
        assert_eq!(
            kind.cmd(),
            UpdateKind::Always,
            "cmd carries the windows path"
        );
        assert_eq!(
            kind.cwd(),
            UpdateKind::Always,
            "cwd resolves a bare exe name"
        );
        assert_eq!(kind.exe(), UpdateKind::Always, "exe is the native fallback");
    }

    // proton hosts every windows exe under one preloader, so ownership has to be
    // read off the command line. this is the translation that makes teardown and
    // "has the game appeared yet" work at all on linux.
    #[cfg(unix)]
    #[test]
    fn a_wine_command_line_translates_to_its_host_path() {
        let root = Path::new("/home/joe/games/spcycle");

        let translated = wine_arg_to_host(
            r"Z:\home\joe\games\spcycle\Prospect\Binaries\Win64\Prospect-Win64-Shipping.exe",
            None,
        );
        let translated = translated.expect("a Z: path is on the host");
        assert_eq!(
            translated,
            Path::new(
                "/home/joe/games/spcycle/Prospect/Binaries/Win64/Prospect-Win64-Shipping.exe"
            )
        );
        assert!(
            is_game_exe(&translated),
            "the leaf name still identifies it"
        );
        assert!(
            owned_by_root(&translated, root),
            "and it is inside the install"
        );

        // lower case drive letters are equally valid, and so are forward slashes:
        // wine accepts both and the loader is not consistent about which it uses
        assert!(wine_arg_to_host(r"z:\tmp\x.exe", None).is_some());
        assert_eq!(
            wine_arg_to_host("Z:/tmp/x.exe", None),
            Some(PathBuf::from("/tmp/x.exe"))
        );

        // anything not on Z: lives inside the prefix, and is not ours to claim
        assert!(wine_arg_to_host(r"C:\windows\system32\winedevice.exe", None).is_none());
        // a native process is judged by its own exe, not by argv[0]
        assert!(wine_arg_to_host("/usr/bin/mongod", None).is_none());
        // the drive letter alone is not a path
        assert!(wine_arg_to_host("Z:", None).is_none());
    }

    // the loader starts the game as a sibling, by name, so the command line
    // carries no directory at all. this is the case that made wait_for_game time
    // out at 120 s while the game was on screen: without cwd there is nothing to
    // resolve the name against, and exe_of falls back to proton's preloader.
    #[cfg(unix)]
    #[test]
    fn a_bare_exe_name_resolves_against_the_process_working_directory() {
        let win64 = std::env::temp_dir().join(format!("spc-bare-{}", std::process::id()));
        std::fs::create_dir_all(&win64).unwrap();
        let exe = win64.join("Prospect-Win64-Shipping.exe");
        std::fs::write(&exe, b"").unwrap();

        let resolved = wine_arg_to_host("Prospect-Win64-Shipping.exe", Some(&win64))
            .expect("the cwd resolves the bare name");
        assert_eq!(resolved, exe);
        assert!(is_game_exe(&resolved));
        assert!(
            owned_by_root(&resolved, &win64),
            "and it is judged to be inside the install"
        );

        // a relative path is the same case with a directory on the front
        let sub = win64.join("Binaries");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("Prospect-Win64-Shipping.exe"), b"").unwrap();
        assert_eq!(
            wine_arg_to_host(r"Binaries\Prospect-Win64-Shipping.exe", Some(&win64)),
            Some(sub.join("Prospect-Win64-Shipping.exe"))
        );

        // without a cwd there is nothing to resolve against
        assert!(wine_arg_to_host("Prospect-Win64-Shipping.exe", None).is_none());
        // and a name that resolves to nothing on disk is not a running process:
        // otherwise any process merely sitting in the install would pass for the game
        assert!(wine_arg_to_host("NotThere.exe", Some(&win64)).is_none());

        std::fs::remove_dir_all(&win64).ok();
    }

    // the exact failure that made the launcher report "the game did not start"
    // while the game was plainly running: proton's preloader is not under the
    // install, so judging by /proc/<pid>/exe finds nothing.
    #[cfg(unix)]
    #[test]
    fn protons_preloader_is_not_mistaken_for_the_game() {
        let root = Path::new("/home/joe/games/spcycle");
        let preloader = Path::new(
            "/home/joe/.steam/steam/steamapps/common/Proton - Experimental/files/lib/wine/x86_64-unix/wine64-preloader",
        );
        assert!(!is_game_exe(preloader));
        assert!(!owned_by_root(preloader, root));
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
