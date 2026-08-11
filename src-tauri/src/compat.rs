#[cfg(unix)]
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompatInfo {
    pub supported: bool,

    pub proton: Vec<String>,

    pub runtimes: std::collections::BTreeMap<String, String>,
    pub steam_root: Option<String>,
}

#[cfg(any(unix, test))]
pub(crate) fn is_flatpak_path(path: &str) -> bool {
    path.contains("/.var/app/")
}

#[cfg(unix)]
pub fn runtime_for(proton: &str, info: &CompatInfo) -> Option<String> {
    let dir = Path::new(proton).parent()?;
    let manifest = std::fs::read_to_string(dir.join("toolmanifest.vdf")).ok()?;
    let appid = required_runtime_appid(&manifest)?;
    let found = info.runtimes.get(&appid).cloned();
    if found.is_none() {
        log::warn!("{proton} wants runtime {appid}, which is not installed; running it directly");
    }
    found
}

#[cfg(any(unix, test))]
fn required_runtime_appid(manifest: &str) -> Option<String> {
    let appid = manifest
        .split("\"require_tool_appid\"")
        .nth(1)?
        .split('"')
        .nth(1)?
        .trim();
    (!appid.is_empty() && appid.chars().all(|c| c.is_ascii_digit())).then(|| appid.to_string())
}

#[cfg(unix)]
fn installed_appid(lib: &Path, installdir: &str) -> Option<String> {
    let needle = format!("\"{installdir}\"");
    std::fs::read_dir(lib.join("steamapps"))
        .ok()?
        .flatten()
        .find_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let appid = name
                .strip_prefix("appmanifest_")?
                .strip_suffix(".acf")?
                .to_string();
            let text = std::fs::read_to_string(entry.path()).ok()?;
            text.lines()
                .any(|l| l.trim_start().starts_with("\"installdir\"") && l.ends_with(&needle))
                .then_some(appid)
        })
}

#[cfg(unix)]
fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".steam/root"));
        roots.push(home.join(".local/share/Steam"));

        roots.push(home.join(".var/app/com.valvesoftware.Steam/data/Steam"));
    }
    roots.push(PathBuf::from("/usr/share/steam"));
    roots.retain(|p| p.join("steamapps").is_dir());

    dedup_by_real_path(&mut roots);
    roots
}

#[cfg(unix)]
fn dedup_by_real_path(paths: &mut Vec<PathBuf>) {
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
}

// posix library paths only, which is why this is linux-only
#[cfg(unix)]
fn library_folders(root: &Path) -> Vec<PathBuf> {
    let vdf = root.join("steamapps/libraryfolders.vdf");
    let Ok(text) = std::fs::read_to_string(&vdf) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("\"path\"") {
            continue;
        }

        if let Some(start) = line.rfind('"').and_then(|_| line.find("\"/")) {
            let rest = &line[start + 1..];
            if let Some(end) = rest.find('"') {
                let p = PathBuf::from(&rest[..end]);
                if p.join("steamapps").is_dir() {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn detect() -> CompatInfo {
    CompatInfo {
        supported: false,
        ..Default::default()
    }
}

#[cfg(unix)]
pub fn detect() -> CompatInfo {
    let mut info = CompatInfo {
        supported: true,
        ..Default::default()
    };

    let mut search: Vec<PathBuf> = Vec::new();
    for root in steam_roots() {
        info.steam_root
            .get_or_insert_with(|| root.display().to_string());
        search.push(root.clone());
        search.extend(library_folders(&root));
    }

    dedup_by_real_path(&mut search);

    for lib in &search {
        let common = lib.join("steamapps/common");
        let Ok(entries) = std::fs::read_dir(&common) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("Proton") || name.contains("proton") {
                let exe = dir.join("proton");
                if exe.is_file() {
                    info.proton.push(exe.display().to_string());
                }
            }
            if name.starts_with("SteamLinuxRuntime") {
                for candidate in ["run", "_v2-entry-point"] {
                    let entry_point = dir.join(candidate);
                    if is_executable(&entry_point) {
                        if let Some(appid) = installed_appid(lib, &name) {
                            info.runtimes
                                .insert(appid, entry_point.display().to_string());
                        }
                        break;
                    }
                }
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        for base in [
            home.join(".steam/root/compatibilitytools.d"),
            home.join(".local/share/Steam/compatibilitytools.d"),
        ] {
            let Ok(entries) = std::fs::read_dir(&base) else {
                continue;
            };
            for entry in entries.flatten() {
                let exe = entry.path().join("proton");
                if exe.is_file() {
                    info.proton.push(exe.display().to_string());
                }
            }
        }
    }

    let mut proton: Vec<PathBuf> = info.proton.iter().map(PathBuf::from).collect();
    dedup_by_real_path(&mut proton);
    info.proton = proton
        .into_iter()
        .map(|p| p.display().to_string())
        .collect();
    info.proton.sort();
    info.proton.dedup();

    info.proton.reverse();
    prefer_native_proton(&mut info.proton);
    info
}

#[cfg(any(unix, test))]
pub(crate) fn prefer_native_proton(proton: &mut [String]) {
    proton.sort_by_key(|p| is_flatpak_path(p));
}

#[cfg(test)]
mod tests {
    use super::required_runtime_appid;

    #[test]
    fn a_build_gets_the_runtime_it_declares_not_whatever_is_installed() {
        let ge = "\"manifest\"\n{\n  \"commandline\" \"/proton run\"\n  \
                  \"require_tool_appid\" \"1391110\"\n}\n";
        assert_eq!(required_runtime_appid(ge).as_deref(), Some("1391110"));
    }

    #[test]
    fn a_runtime_this_launcher_has_never_heard_of_still_resolves() {
        let newer = "\"manifest\"\n{\n  \"require_tool_appid\" \"4183110\"\n}\n";
        assert_eq!(required_runtime_appid(newer).as_deref(), Some("4183110"));
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "reads the Steam install on this machine"]
    fn every_installed_proton_resolves_to_a_runtime_that_exists() {
        let info = super::detect();
        assert!(!info.proton.is_empty(), "no proton builds found to check");
        for proton in &info.proton {
            if let Some(run) = super::runtime_for(proton, &info) {
                assert!(
                    std::path::Path::new(&run).is_file(),
                    "{proton} resolved to a missing runtime entry point: {run}"
                );
                println!("{proton}\n  -> {run}");
            } else {
                println!("{proton}\n  -> no runtime (runs directly)");
            }
        }
    }

    #[test]
    fn a_build_that_names_no_runtime_runs_directly() {
        assert_eq!(
            required_runtime_appid("\"manifest\"\n{\n  \"commandline\" \"/proton run\"\n}\n"),
            None
        );
    }

    #[test]
    fn a_native_proton_outranks_flatpaks() {
        let mut found = vec![
            "/home/u/.var/app/com.valvesoftware.Steam/data/Steam/steamapps/common/Proton - Experimental/proton".to_string(),
            "/home/u/.steam/steam/steamapps/common/Proton 9.0/proton".to_string(),
            "/home/u/.local/share/Steam/compatibilitytools.d/GE-Proton/proton".to_string(),
        ];
        super::prefer_native_proton(&mut found);
        assert!(
            !super::is_flatpak_path(&found[0]),
            "a flatpak proton must never be first: {found:?}"
        );
        assert!(
            super::is_flatpak_path(found.last().unwrap()),
            "flatpak belongs last"
        );
        // order within the native group is preserved
        assert!(
            found[0].contains(".steam") && found[1].contains(".local"),
            "{found:?}"
        );
    }

    #[test]
    fn only_flatpak_paths_are_treated_as_flatpak() {
        assert!(super::is_flatpak_path(
            "/home/u/.var/app/com.valvesoftware.Steam/x/proton"
        ));
        assert!(!super::is_flatpak_path(
            "/home/u/.steam/steam/steamapps/common/Proton/proton"
        ));
        assert!(!super::is_flatpak_path("/opt/proton/varapp/proton"));
    }
}

#[cfg(all(unix, test))]
mod smoke {
    // prints what this machine actually has; not an assertion of presence
    #[test]
    #[ignore = "prints local Steam/Proton detection; run with --nocapture"]
    fn detection_on_this_machine() {
        let info = super::detect();
        println!("supported:  {}", info.supported);
        println!("steam root: {:?}", info.steam_root);
        println!("proton:     {} build(s)", info.proton.len());
        for p in &info.proton {
            println!("  {p}");
        }
        println!("runtimes:   {:?}", info.runtimes);

        // a proton build we listed must be a file we can actually run
        for p in &info.proton {
            assert!(
                std::path::Path::new(p).is_file(),
                "{p} was listed but is not a file"
            );
        }
        // any proton found means we must know where steam lives
        if !info.proton.is_empty() {
            assert!(
                info.steam_root.is_some(),
                "proton was found without a steam root"
            );
        }
    }
}
