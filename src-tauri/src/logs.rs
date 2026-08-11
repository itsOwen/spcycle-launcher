use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

// mongod's log grows without bound; the ui shows a few hundred lines at most
const MAX_TAIL_BYTES: u64 = 256 * 1024;

pub fn tail(path: &Path, lines: usize) -> String {
    match read_tail(path, lines) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => format!("could not read {}: {e}", path.display()),
    }
}

fn read_tail(path: &Path, lines: usize) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(MAX_TAIL_BYTES);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }

    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf)?;

    // a truncated multi-byte char at the seek point must not lose the whole file
    let text = String::from_utf8_lossy(&buf);
    // drop the first, possibly partial, line only when we actually seeked
    let text = if start > 0 {
        text.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
    } else {
        &text
    };

    let kept: Vec<&str> = text.lines().rev().take(lines).collect();
    Ok(kept.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_returns_the_last_n_lines_in_order() {
        let dir = std::env::temp_dir().join(format!("spc-logs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.log");
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();

        assert_eq!(tail(&path, 2), "three\nfour");
        assert_eq!(tail(&path, 99), "one\ntwo\nthree\nfour");

        std::fs::remove_dir_all(&dir).ok();
    }

    // a missing log is normal before first launch
    #[test]
    fn a_missing_log_is_empty_not_an_error() {
        let path = std::env::temp_dir().join("spc-does-not-exist-abc123.log");
        assert_eq!(tail(&path, 10), "");
    }
}
