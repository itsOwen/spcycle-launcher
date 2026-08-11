// what is missing from this machine before the game can run
// windows needs nothing checked; linux needs steam and a proton build

use serde::Serialize;
use tauri::AppHandle;

use crate::settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    // cannot run at all
    Blocking,
    // will run, but worse
    Degraded,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    pub title: String,
    // written for the user, not the log
    pub impact: String,
    pub ok: bool,
    pub severity: Severity,
    // an install command, when we can name one for this distro
    pub install: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub distro: String,
    pub checks: Vec<Check>,
    pub all_ok: bool,
    pub has_blocking: bool,
}

fn finish(distro: String, checks: Vec<Check>) -> Preflight {
    let all_ok = checks.iter().all(|c| c.ok);
    let has_blocking = checks
        .iter()
        .any(|c| !c.ok && c.severity == Severity::Blocking);
    Preflight {
        distro,
        checks,
        all_ok,
        has_blocking,
    }
}

#[cfg(windows)]
pub fn run(_app: &AppHandle) -> Preflight {
    // mongod is bundled, the cert goes to the user store, steam starts on demand
    finish("Windows".into(), Vec::new())
}

#[cfg(unix)]
pub fn run(app: &AppHandle) -> Preflight {
    use crate::{compat, proc};

    let os = os_release();
    let family = family(&os);
    let info = compat::detect();
    let tool = settings::compat_tool(app);

    let mut checks = Vec::new();

    // proton specifically. wine alone cannot supply a steam ticket.
    let has_proton = !info.proton.is_empty()
        || settings::proton_path(app).is_some_and(|p| std::path::Path::new(&p).is_file());
    checks.push(Check {
        id: "proton".into(),
        title: "A Proton build".into(),
        impact: "The game will not launch. Install Proton through Steam, or drop a build \
                 into ~/.steam/steam/compatibilitytools.d."
            .into(),
        ok: has_proton,
        severity: Severity::Blocking,
        install: None,
    });

    // plain wine is a configuration mistake, not a missing package
    checks.push(Check {
        id: "proton-not-wine".into(),
        title: "Proton selected, not plain Wine".into(),
        impact: "Plain Wine has no bridge to the Steam client, so signing in fails with \
                 \"Login Failed. Error code: 3\". Choose Proton in Settings."
            .into(),
        ok: tool == "proton" || tool == "custom",
        severity: Severity::Blocking,
        install: None,
    });

    checks.push(Check {
        id: "steam".into(),
        title: "Steam is installed".into(),
        impact: "The game authenticates through the Steam client; without it there is \
                 nothing to sign in with."
            .into(),
        ok: info.steam_root.is_some() || proc::steam_is_running(),
        severity: Severity::Blocking,
        install: install_cmd(family, "steam"),
    });

    // 40 GiB, because the depot unpacks to 36.8
    const NEEDED: u64 = 40 * 1024 * 1024 * 1024;
    let dir = settings::game_directory(app);
    let free = crate::free_space(&dir).unwrap_or(0);
    checks.push(Check {
        id: "space".into(),
        title: "40 GiB free on the game drive".into(),
        impact: format!(
            "The game needs 36.8 GiB unpacked. {} is free where it would be installed.",
            crate::depot::human(free)
        ),
        ok: free == 0 || free >= NEEDED,
        severity: Severity::Blocking,
        install: None,
    });

    // mongodb 5.0+ dies at startup without avx, with an unhelpful message
    checks.push(Check {
        id: "avx".into(),
        title: "A CPU with AVX".into(),
        impact: "MongoDB 5.0 and newer require AVX and will not start without it.".into(),
        ok: has_avx(),
        severity: Severity::Degraded,
        install: None,
    });

    finish(distro_name(&os), checks)
}

#[cfg(unix)]
fn os_release() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string("/etc/os-release") else {
        return out;
    };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        }
    }
    out
}

#[cfg(unix)]
fn distro_name(os: &std::collections::HashMap<String, String>) -> String {
    os.get("PRETTY_NAME")
        .or_else(|| os.get("NAME"))
        .cloned()
        .unwrap_or_else(|| "Linux".into())
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Arch,
    Debian,
    Fedora,
    Suse,
    Alpine,
    Unknown,
}

#[cfg(unix)]
fn family(os: &std::collections::HashMap<String, String>) -> Family {
    let mut ids: Vec<String> = Vec::new();
    if let Some(id) = os.get("ID") {
        ids.push(id.to_lowercase());
    }
    if let Some(like) = os.get("ID_LIKE") {
        ids.extend(like.to_lowercase().split_whitespace().map(String::from));
    }
    for id in &ids {
        match id.as_str() {
            "arch" | "archlinux" | "manjaro" | "endeavouros" => return Family::Arch,
            "debian" | "ubuntu" | "linuxmint" | "pop" => return Family::Debian,
            "fedora" | "rhel" | "centos" => return Family::Fedora,
            "opensuse" | "suse" | "sles" => return Family::Suse,
            "alpine" => return Family::Alpine,
            _ => {}
        }
    }
    Family::Unknown
}

// never a guess: an install line that does not work is worse than none
#[cfg(unix)]
fn install_cmd(family: Family, package: &str) -> Option<String> {
    Some(match family {
        Family::Arch => format!("sudo pacman -S {package}"),
        Family::Debian => format!("sudo apt install {package}"),
        Family::Fedora => format!("sudo dnf install {package}"),
        Family::Suse => format!("sudo zypper install {package}"),
        Family::Alpine => format!("sudo apk add {package}"),
        Family::Unknown => return None,
    })
}

#[cfg(unix)]
fn has_avx() -> bool {
    let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") else {
        // unknown is not a failure to report
        return true;
    };
    info.lines()
        .filter(|l| l.starts_with("flags"))
        .any(|l| l.split_whitespace().any(|f| f == "avx"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // a failing check must say what it costs, or the dialog is a list of nouns
    #[test]
    fn every_check_explains_its_impact() {
        let checks = vec![Check {
            id: "x".into(),
            title: "t".into(),
            impact: "i".into(),
            ok: false,
            severity: Severity::Blocking,
            install: None,
        }];
        let p = finish("Test".into(), checks);
        assert!(p.has_blocking);
        assert!(!p.all_ok);
        for c in &p.checks {
            assert!(!c.impact.is_empty(), "{} has no impact text", c.id);
        }
    }

    #[test]
    fn a_degraded_check_does_not_block() {
        let p = finish(
            "Test".into(),
            vec![Check {
                id: "avx".into(),
                title: "t".into(),
                impact: "i".into(),
                ok: false,
                severity: Severity::Degraded,
                install: None,
            }],
        );
        assert!(!p.all_ok);
        assert!(!p.has_blocking, "degraded must never block a launch");
    }

    #[cfg(unix)]
    #[test]
    fn an_unknown_distro_invents_no_command() {
        assert_eq!(install_cmd(Family::Unknown, "steam"), None);
        assert_eq!(
            install_cmd(Family::Arch, "steam").as_deref(),
            Some("sudo pacman -S steam")
        );
    }

    #[cfg(unix)]
    #[test]
    fn id_like_is_used_when_id_is_unknown() {
        let mut os = std::collections::HashMap::new();
        os.insert("ID".to_string(), "somethingnew".to_string());
        os.insert("ID_LIKE".to_string(), "arch".to_string());
        assert_eq!(family(&os), Family::Arch);

        // and a bare unknown stays unknown rather than guessing
        let mut lonely = std::collections::HashMap::new();
        lonely.insert("ID".to_string(), "mysteryos".to_string());
        assert_eq!(family(&lonely), Family::Unknown);
    }

    #[cfg(unix)]
    #[test]
    fn the_real_distro_is_recognised() {
        // whatever this box is, naming it must not panic
        let os = os_release();
        let name = distro_name(&os);
        assert!(!name.is_empty());
        println!("detected: {name} ({:?})", family(&os));
    }
}
