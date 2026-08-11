// wine's trusted root store, backed by the prefix registry.
//
// each certificate is one subkey under HKCU\Software\Microsoft\SystemCertificates\Root\Certificates,
// named with the uppercase hex sha-1 of
// the der, holding a REG_BINARY value called Blob. wine recomputes that hash on
// load and silently drops a certificate whose subkey name does not match.

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use super::{CertError, Leaf};

// the property that holds the encoded certificate
const CERT_CERT_PROP_ID: u32 = 32;

// wine writes 1 here for every property it serialises
const PROP_UNKNOWN: u32 = 1;

const ROOT_KEY: &str = r"Software\Microsoft\SystemCertificates\Root\Certificates";

// serialise a certificate the way wine's registry store expects it
pub fn serialize_blob(der: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + der.len());
    out.extend_from_slice(&CERT_CERT_PROP_ID.to_le_bytes());
    out.extend_from_slice(&PROP_UNKNOWN.to_le_bytes());
    out.extend_from_slice(&(der.len() as u32).to_le_bytes());
    out.extend_from_slice(der);
    out
}

// a .reg body adding the certificate under its thumbprint
pub fn reg_script(thumbprint_hex: &str, blob: &[u8]) -> String {
    let mut s = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
    s.push_str(&format!(
        "[HKEY_CURRENT_USER\\{ROOT_KEY}\\{thumbprint_hex}]\r\n"
    ));
    s.push_str("\"Blob\"=hex:");

    // reg files wrap long values with a trailing backslash, 25 bytes a line
    for (i, b) in blob.iter().enumerate() {
        if i > 0 {
            s.push(',');
            if i % 25 == 0 {
                s.push_str("\\\r\n  ");
            }
        }
        s.push_str(&format!("{b:02x}"));
    }
    s.push_str("\r\n\r\n");
    s
}

// utf-16le with a bom, which is what reg.exe import expects
fn utf16le(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn prefix_dir(prefix_root: &Path) -> PathBuf {
    prefix_root.join("compatdata").join("pfx")
}

fn user_reg(prefix_root: &Path) -> PathBuf {
    prefix_dir(prefix_root).join("user.reg")
}

// reads user.reg rather than spawning reg.exe query; a false negative only
// causes an idempotent re-import
pub fn present(thumbprint_hex: &str, prefix_root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(user_reg(prefix_root)) else {
        return false;
    };
    // wine escapes backslashes in key paths
    let needle = format!("{}\\\\{}", ROOT_KEY.replace('\\', "\\\\"), thumbprint_hex);
    text.contains(&needle)
}

pub async fn import(app: &AppHandle, leaf: &Leaf, prefix_root: &Path) -> Result<(), CertError> {
    let blob = serialize_blob(&leaf.der);
    let script = reg_script(&leaf.hex(), &blob);

    let temp = prefix_dir(prefix_root)
        .join("drive_c")
        .join("windows")
        .join("temp");
    std::fs::create_dir_all(&temp)
        .map_err(|e| CertError::StoreFailed(format!("could not write into the prefix: {e}")))?;

    let host_path = temp.join("spcycle-cert.reg");
    std::fs::write(&host_path, utf16le(&script))
        .map_err(|e| CertError::StoreFailed(format!("could not write the registry script: {e}")))?;

    // the prefix's own view of that file, deterministic, so no path translation
    let win_path = r"C:\windows\temp\spcycle-cert.reg";

    let reg = Path::new("reg.exe");
    let mut cmd = crate::launch::wrap_exe(app, reg, prefix_root, false)
        .map_err(|e| CertError::StoreFailed(e.to_string()))?;
    cmd.arg("import").arg(win_path);

    let out = cmd
        .output()
        .await
        .map_err(|e| CertError::StoreFailed(format!("could not run reg.exe: {e}")))?;

    let _ = std::fs::remove_file(&host_path);

    if !out.status.success() {
        return Err(CertError::StoreFailed(format!(
            "reg.exe import failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

// delete exactly this thumbprint's subkey from the prefix
pub async fn remove(
    app: &AppHandle,
    thumbprint_hex: &str,
    prefix_root: &Path,
) -> Result<bool, CertError> {
    if !prefix_dir(prefix_root).is_dir() {
        return Ok(false);
    }
    let key = format!("HKCU\\{ROOT_KEY}\\{thumbprint_hex}");

    let reg = Path::new("reg.exe");
    let mut cmd = crate::launch::wrap_exe(app, reg, prefix_root, false)
        .map_err(|e| CertError::StoreFailed(e.to_string()))?;
    cmd.arg("delete").arg(&key).arg("/f");

    let out = cmd
        .output()
        .await
        .map_err(|e| CertError::StoreFailed(format!("could not run reg.exe: {e}")))?;
    // a missing key is reported as a failure; that is not an error for us
    Ok(out.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    // the exact 12 bytes wine's deserialiser reads before the certificate
    #[test]
    fn the_blob_header_is_propid_32_then_1_then_length() {
        let der = [0x30u8, 0x03, 0xAA, 0xBB, 0xCC];
        let blob = serialize_blob(&der);

        assert_eq!(blob.len(), 12 + der.len());
        assert_eq!(&blob[0..4], &32u32.to_le_bytes(), "propID must be 32");
        assert_eq!(
            &blob[4..8],
            &1u32.to_le_bytes(),
            "the unknown field is always 1"
        );
        assert_eq!(&blob[8..12], &(der.len() as u32).to_le_bytes(), "then cb");
        assert_eq!(&blob[12..], &der, "then the certificate itself");
    }

    // lowercase hex means the certificate is written and then silently ignored
    #[test]
    fn the_subkey_name_is_uppercase_hex() {
        let blob = serialize_blob(&[0x30, 0x00]);
        let script = reg_script("AABBCCDD00112233445566778899AABBCCDDEEFF", &blob);

        assert!(
            script.contains(r"[HKEY_CURRENT_USER\Software\Microsoft\SystemCertificates\Root\Certificates\AABBCCDD00112233445566778899AABBCCDDEEFF]"),
            "wrong key path:\n{script}"
        );
        assert!(
            !script.contains("aabbccdd"),
            "the subkey must not be lowercased"
        );
    }

    #[test]
    fn the_reg_script_declares_a_binary_blob() {
        let script = reg_script("AB", &serialize_blob(&[0x30, 0x01, 0x02]));
        assert!(script.starts_with("Windows Registry Editor Version 5.00"));
        assert!(script.contains("\"Blob\"=hex:20,00,00,00,01,00,00,00,03,00,00,00,30,01,02"));
    }

    // long certificates must wrap, or reg.exe rejects the line
    #[test]
    fn long_values_are_wrapped_with_continuations() {
        let der = vec![0x41u8; 200];
        let script = reg_script("AA", &serialize_blob(&der));
        assert!(script.contains("\\\r\n  "), "no line continuations emitted");
        for line in script.lines() {
            assert!(line.len() < 1024, "a line grew too long for reg.exe");
        }
    }

    #[test]
    fn the_reg_file_is_utf16le_with_a_bom() {
        let bytes = utf16le("AB");
        assert_eq!(&bytes[0..2], &[0xFF, 0xFE], "missing BOM");
        assert_eq!(&bytes[2..], &[b'A', 0x00, b'B', 0x00]);
    }

    // catches the typo'd constant next to the real one
    #[test]
    fn the_root_key_path_has_no_duplicated_segment() {
        assert!(!ROOT_KEY.contains("Microsoft\\Microsoft"));
        assert_eq!(
            ROOT_KEY,
            r"Software\Microsoft\SystemCertificates\Root\Certificates"
        );
    }

    #[test]
    fn present_is_false_when_there_is_no_prefix() {
        let nowhere = std::env::temp_dir().join("spc-no-prefix-here-zz");
        assert!(!present("AABB", &nowhere));
    }
}
