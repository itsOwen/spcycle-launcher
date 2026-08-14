use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::Serialize;
use tauri::{path::BaseDirectory, AppHandle, Manager};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSupport {
    pub video: bool,
    pub audio_sink: bool,
    pub h264: bool,
}

static MEDIA_URL: OnceLock<String> = OnceLock::new();

pub fn serve(app: &AppHandle) -> Result<String, String> {
    if let Some(url) = MEDIA_URL.get() {
        return Ok(url.clone());
    }

    let path = app
        .path()
        .resolve("backdrop.mp4", BaseDirectory::Resource)
        .map_err(|e| format!("Could not resolve the backdrop video: {e}"))?;
    if !path.is_file() {
        return Err(format!("The backdrop video is missing: {}", path.display()));
    }

    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| format!("Could not bind the backdrop media server: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Could not read the backdrop media address: {e}"))?
        .port();

    std::thread::Builder::new()
        .name("backdrop-media".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let path = path.clone();
                        let _ = std::thread::Builder::new()
                            .name("backdrop-media-request".into())
                            .spawn(move || {
                                if let Err(e) = respond(stream, &path) {
                                    log::debug!("backdrop media request failed: {e}");
                                }
                            });
                    }
                    Err(e) => log::debug!("backdrop media listener failed: {e}"),
                }
            }
        })
        .map_err(|e| format!("Could not start the backdrop media server: {e}"))?;

    let url = format!("http://127.0.0.1:{port}/backdrop.mp4");
    let _ = MEDIA_URL.set(url.clone());
    Ok(MEDIA_URL.get().cloned().unwrap_or(url))
}

fn respond(mut stream: TcpStream, path: &Path) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;

    let (method, target, range) = {
        let mut reader = BufReader::new(&stream);
        let mut first = String::new();
        reader.read_line(&mut first)?;
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let mut range = None;
        let mut bytes = first.len();

        loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line)?;
            bytes += read;
            if read == 0 || line == "\r\n" || line == "\n" || bytes > 16 * 1024 {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("range") {
                    range = Some(value.trim().to_string());
                }
            }
        }
        (method, target, range)
    };

    if !matches!(method.as_str(), "GET" | "HEAD") || target != "/backdrop.mp4" {
        stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    }

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let selected = match range {
        Some(value) => match parse_range(&value, len) {
            Some(value) => Some(value),
            None => {
                write!(
                    stream,
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )?;
                return Ok(());
            }
        },
        None => None,
    };

    let (status, start, end) = match selected {
        Some((start, end)) => ("206 Partial Content", start, end),
        None => ("200 OK", 0, len.saturating_sub(1)),
    };
    let count = if len == 0 { 0 } else { end - start + 1 };

    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nContent-Length: {count}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n"
    )?;
    if selected.is_some() {
        write!(stream, "Content-Range: bytes {start}-{end}/{len}\r\n")?;
    }
    stream.write_all(b"\r\n")?;

    if method == "GET" && count > 0 {
        file.seek(SeekFrom::Start(start))?;
        std::io::copy(&mut file.take(count), &mut stream)?;
    }
    Ok(())
}

fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    let value = value.strip_prefix("bytes=")?;
    if len == 0 || value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?.min(len);
        return (suffix > 0).then_some((len - suffix, len - 1));
    }
    let start = start.parse::<u64>().ok()?;
    if start >= len {
        return None;
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().ok()?.min(len - 1)
    };
    (end >= start).then_some((start, end))
}

fn plugin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for var in [
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        "GST_PLUGIN_PATH_1_0",
        "GST_PLUGIN_PATH",
    ] {
        if let Some(v) = std::env::var_os(var) {
            dirs.extend(std::env::split_paths(&v));
        }
    }
    let arch = std::env::consts::ARCH;
    dirs.push(PathBuf::from(format!(
        "/usr/lib/{arch}-linux-gnu/gstreamer-1.0"
    )));
    dirs.push(PathBuf::from("/usr/lib64/gstreamer-1.0"));
    dirs.push(PathBuf::from("/usr/lib/gstreamer-1.0"));
    dirs.push(PathBuf::from("/usr/local/lib/gstreamer-1.0"));
    dirs.retain(|d| d.is_dir());
    dirs
}

fn has_any(dirs: &[PathBuf], names: &[&str]) -> bool {
    dirs.iter().any(|d| {
        std::fs::read_dir(d)
            .map(|entries| {
                entries.flatten().any(|e| {
                    let f = e.file_name().to_string_lossy().to_string();
                    names.iter().any(|n| f.contains(n))
                })
            })
            .unwrap_or(false)
    })
}

pub fn detect_cached() -> MediaSupport {
    static CACHE: std::sync::OnceLock<MediaSupport> = std::sync::OnceLock::new();
    *CACHE.get_or_init(detect)
}

pub fn detect() -> MediaSupport {
    // windows decodes h264 in the webview itself
    if cfg!(not(target_os = "linux")) {
        return MediaSupport {
            video: true,
            audio_sink: true,
            h264: true,
        };
    }

    let dirs = plugin_dirs();
    let audio_sink = has_any(&dirs, &["libgstautodetect"])
        && has_any(
            &dirs,
            &["libgstpulse", "libgstalsa", "libgstpipewire", "libgstoss4"],
        );
    let h264 = has_any(
        &dirs,
        &["libgstlibav", "libgstopenh264", "libgstvaapi", "libgstva."],
    );

    MediaSupport {
        video: audio_sink && h264,
        audio_sink,
        h264,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a decoder with nowhere to send the audio still cannot play the file
    #[test]
    fn video_needs_both_a_sink_and_a_decoder() {
        let s = detect();
        assert_eq!(s.video, s.audio_sink && s.h264);
    }

    #[test]
    fn plugin_dirs_are_all_real() {
        assert!(plugin_dirs().iter().all(|d| d.is_dir()));
    }

    #[test]
    fn ranges_cover_webkit_probe_and_seek_forms() {
        assert_eq!(parse_range("bytes=0-1", 100), Some((0, 1)));
        assert_eq!(parse_range("bytes=40-", 100), Some((40, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 100), None);
    }
}
