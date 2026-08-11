// getting the local server's tls certificate trusted by the game.
//
// the game's http stack is openssl, but it seeds its roots from the windows
// certificate store, and the depot ships no cacert.pem. so the certificate goes
// into a root store: the current user's on windows, wine's registry-backed one
// inside the game prefix on linux.
//
// generate_ssl.exe writes the pkcs#12 with NoEncryption(), so the der can be
// lifted out with a short asn.1 walk instead of a pkcs#12 library.

#[cfg(windows)]
mod windows;

#[cfg(unix)]
mod wine;

use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};
use tauri::AppHandle;

use crate::settings;

// a frozen cpython generating an rsa-4096 key, slower still under wine
const GENERATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

// how long to wait for certificate.pfx to appear after the tool exits
const FILE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

const PFX: &str = "certificate.pfx";
const GENERATOR: &str = "generate_ssl.exe";
const STAMP: &str = "cert.thumbprint";

#[derive(Debug, thiserror::Error)]
pub enum CertError {
    #[error("The certificate generator is missing. Reinstall the components.")]
    NoGenerator,
    #[error("Could not generate the certificate: {0}")]
    GenerateFailed(String),
    #[error("The certificate generator did not produce {PFX}.")]
    NoPfx,
    #[error("The certificate file is not in the expected format: {0}")]
    BadPfx(&'static str),
    #[error("Could not add the certificate to the trust store: {0}")]
    StoreFailed(String),
}

// the leaf certificate in der, plus its sha-1 thumbprint
#[derive(Debug, Clone)]
pub struct Leaf {
    pub der: Vec<u8>,
    pub thumbprint: [u8; 20],
}

impl Leaf {
    // uppercase hex: wine drops a certificate whose subkey name does not match
    pub fn hex(&self) -> String {
        self.thumbprint.iter().map(|b| format!("{b:02X}")).collect()
    }
}

pub fn pfx_path(app: &AppHandle) -> PathBuf {
    settings::server_dir(app).join(PFX)
}

fn stamp_path(app: &AppHandle) -> PathBuf {
    settings::app_data(app).join(STAMP)
}

// makes uninstall safe: removes exactly the certificate it added, never a search
pub fn trusted_thumbprint(app: &AppHandle) -> Option<String> {
    std::fs::read_to_string(stamp_path(app))
        .ok()
        .map(|s| s.trim().to_uppercase())
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

// pull the leaf der out of an unencrypted pkcs#12
fn der_from_pfx(pfx: &[u8]) -> Result<Vec<u8>, CertError> {
    // oid 1.2.840.113549.1.9.22.1, der-encoded
    const CERT_BAG_OID: &[u8] = &[
        0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x16, 0x01,
    ];

    let at = pfx
        .windows(CERT_BAG_OID.len())
        .position(|w| w == CERT_BAG_OID)
        .ok_or(CertError::BadPfx("no certificate bag found"))?;

    let mut i = at + CERT_BAG_OID.len();

    // [0] explicit wrapper
    if pfx.get(i) != Some(&0xA0) {
        return Err(CertError::BadPfx("the certificate bag is not [0]-wrapped"));
    }
    i += 1;
    let (_, next) = read_len(pfx, i)?;
    i = next;

    // the OCTET STRING holding the certificate
    if pfx.get(i) != Some(&0x04) {
        return Err(CertError::BadPfx(
            "expected an OCTET STRING of certificate bytes",
        ));
    }
    i += 1;
    let (len, next) = read_len(pfx, i)?;
    i = next;

    let der = pfx.get(i..i + len).ok_or(CertError::BadPfx(
        "the certificate runs past the end of the file",
    ))?;

    // a certificate is a SEQUENCE; anything else means we mis-walked
    if der.first() != Some(&0x30) {
        return Err(CertError::BadPfx(
            "the extracted bytes are not a certificate",
        ));
    }
    Ok(der.to_vec())
}

// der length at i, returning the length and the offset of the content
fn read_len(buf: &[u8], i: usize) -> Result<(usize, usize), CertError> {
    let first = *buf.get(i).ok_or(CertError::BadPfx("truncated length"))?;
    if first < 0x80 {
        return Ok((first as usize, i + 1));
    }
    let n = (first & 0x7F) as usize;
    if n == 0 || n > 4 {
        return Err(CertError::BadPfx("unsupported length encoding"));
    }
    let mut len = 0usize;
    for k in 0..n {
        let b = *buf
            .get(i + 1 + k)
            .ok_or(CertError::BadPfx("truncated long-form length"))?;
        len = (len << 8) | b as usize;
    }
    Ok((len, i + 1 + n))
}

fn leaf_from_pfx(pfx: &[u8]) -> Result<Leaf, CertError> {
    let der = der_from_pfx(pfx)?;
    let thumbprint: [u8; 20] = Sha1::digest(&der).into();
    Ok(Leaf { der, thumbprint })
}

// prefix_root is linux-only: the generator is a windows binary and must run
// inside the same prefix as the server and the game
pub async fn ensure_cert(app: &AppHandle, prefix_root: &Path) -> Result<Leaf, CertError> {
    let pfx = pfx_path(app);

    if !pfx.is_file() {
        let dir = settings::server_dir(app);
        let generator = dir.join(GENERATOR);
        if !generator.is_file() {
            return Err(CertError::NoGenerator);
        }
        log::info!("generating the server certificate");
        run_generator(app, &generator, &dir, prefix_root).await?;
        wait_for_file(&pfx, FILE_TIMEOUT).await?;
    }

    let bytes = std::fs::read(&pfx).map_err(|_| CertError::NoPfx)?;
    leaf_from_pfx(&bytes)
}

async fn run_generator(
    app: &AppHandle,
    generator: &Path,
    cwd: &Path,
    prefix_root: &Path,
) -> Result<(), CertError> {
    use tokio::io::AsyncWriteExt;

    let mut cmd = crate::launch::wrap_exe(app, generator, prefix_root, false)
        .map_err(|e| CertError::GenerateFailed(e.to_string()))?;
    cmd.current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        CertError::GenerateFailed(format!("could not run {}: {e}", generator.display()))
    })?;

    // the tool ends with input(), so it sits there forever unless answered
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"\n").await;
        let _ = stdin.flush().await;
    }

    let out = tokio::time::timeout(GENERATE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            CertError::GenerateFailed(format!(
                "it did not finish within {}s",
                GENERATE_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| CertError::GenerateFailed(e.to_string()))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stdout.lines().chain(stderr.lines()) {
        log::info!("[generate_ssl] {line}");
    }

    // some builds return non-zero after the prompt yet still wrote the file
    if !out.status.success() {
        log::warn!("generate_ssl exited with {}", out.status);
    }
    Ok(())
}

async fn wait_for_file(path: &Path, within: std::time::Duration) -> Result<(), CertError> {
    let deadline = std::time::Instant::now() + within;
    while !path.is_file() {
        if std::time::Instant::now() >= deadline {
            return Err(CertError::NoPfx);
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(())
}

// idempotent
pub async fn ensure_trusted(
    app: &AppHandle,
    leaf: &Leaf,
    prefix_root: &Path,
) -> Result<(), CertError> {
    let hex = leaf.hex();

    // the stamp is the fast path: after the first launch this is a file read
    if trusted_thumbprint(app).as_deref() == Some(hex.as_str())
        && already_trusted(&hex, prefix_root)
    {
        return Ok(());
    }

    add_to_store(app, leaf, prefix_root).await?;
    std::fs::write(stamp_path(app), &hex)
        .map_err(|e| CertError::StoreFailed(format!("could not record the thumbprint: {e}")))?;
    log::info!("server certificate {hex} is trusted");
    Ok(())
}

#[cfg(windows)]
fn already_trusted(hex: &str, _prefix_root: &Path) -> bool {
    windows::present(hex)
}

#[cfg(unix)]
fn already_trusted(hex: &str, prefix_root: &Path) -> bool {
    wine::present(hex, prefix_root)
}

#[cfg(windows)]
async fn add_to_store(_app: &AppHandle, leaf: &Leaf, _prefix_root: &Path) -> Result<(), CertError> {
    windows::add_to_current_user_root(&leaf.der)
}

#[cfg(unix)]
async fn add_to_store(app: &AppHandle, leaf: &Leaf, prefix_root: &Path) -> Result<(), CertError> {
    wine::import(app, leaf, prefix_root).await
}

// by thumbprint only, never a search
pub async fn untrust(app: &AppHandle, prefix_root: &Path) -> Result<bool, CertError> {
    let Some(hex) = trusted_thumbprint(app) else {
        return Ok(false);
    };
    #[cfg(windows)]
    let removed = {
        let _ = prefix_root;
        windows::remove(&hex)?
    };
    #[cfg(unix)]
    let removed = wine::remove(app, &hex, prefix_root).await?;

    let _ = std::fs::remove_file(stamp_path(app));
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // a minimal pkcs#12-shaped buffer: certbag oid, [0] wrapper, octet string
    fn fake_pfx(cert_der: &[u8]) -> Vec<u8> {
        let mut v = vec![0xAA; 40]; // leading noise, as a real file has
        v.extend_from_slice(&[
            0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x16, 0x01,
        ]);
        // [0] wrapper, short form
        v.push(0xA0);
        v.push((cert_der.len() + 2) as u8);
        // OCTET STRING
        v.push(0x04);
        v.push(cert_der.len() as u8);
        v.extend_from_slice(cert_der);
        v.extend_from_slice(&[0xBB; 16]); // trailing noise
        v
    }

    fn a_certificate() -> Vec<u8> {
        // SEQUENCE { 20 bytes }
        let mut der = vec![0x30, 0x14];
        der.extend_from_slice(&[0x42; 20]);
        der
    }

    #[test]
    fn der_is_scraped_from_an_unencrypted_pfx() {
        let cert = a_certificate();
        let leaf = leaf_from_pfx(&fake_pfx(&cert)).expect("the bag should be found");
        assert_eq!(leaf.der, cert);

        // the thumbprint must be sha-1 over the der, which both stores key on
        let expected: [u8; 20] = Sha1::digest(&cert).into();
        assert_eq!(leaf.thumbprint, expected);
    }

    // lowercase hex means the certificate is silently lost
    #[test]
    fn the_thumbprint_is_uppercase_hex_of_forty_chars() {
        let leaf = leaf_from_pfx(&fake_pfx(&a_certificate())).unwrap();
        let hex = leaf.hex();
        assert_eq!(hex.len(), 40);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_lowercase()),
            "{hex} must be uppercase hex"
        );
    }

    #[test]
    fn a_pfx_without_a_certificate_bag_is_refused() {
        assert!(leaf_from_pfx(b"nothing to see here").is_err());
    }

    #[test]
    fn a_bag_that_does_not_hold_a_certificate_is_refused() {
        // an OCTET STRING whose contents are not a SEQUENCE
        let err = leaf_from_pfx(&fake_pfx(&[0x05, 0x00])).expect_err("must not accept a non-cert");
        assert!(matches!(err, CertError::BadPfx(_)), "{err}");
    }

    #[test]
    fn long_form_lengths_are_understood() {
        // a 300-byte certificate forces the two-byte long form
        let mut cert = vec![0x30, 0x82, 0x01, 0x2C];
        cert.extend_from_slice(&[0x7; 300]);

        let mut v = vec![0xAA; 8];
        v.extend_from_slice(&[
            0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x16, 0x01,
        ]);
        v.push(0xA0);
        v.extend_from_slice(&[0x82, 0x01, 0x38]); // [0], long form
        v.push(0x04);
        v.extend_from_slice(&[0x82, 0x01, 0x30]); // OCTET STRING, long form
        v.extend_from_slice(&cert);

        let leaf = leaf_from_pfx(&v).expect("long-form lengths must parse");
        assert_eq!(leaf.der.len(), 304);
    }

    #[test]
    fn a_thumbprint_stamp_must_be_forty_hex_chars() {
        // a junk stamp must not be treated as a certificate we own
        for junk in ["", "xyz", &"g".repeat(40), &"a".repeat(39)] {
            assert!(
                !(junk.len() == 40 && junk.bytes().all(|b| b.is_ascii_hexdigit())),
                "{junk:?} should not pass the stamp check"
            );
        }
    }
}
