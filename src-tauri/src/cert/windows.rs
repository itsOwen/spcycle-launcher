// the current user's trusted root store, via cryptoapi rather than powershell.
// per-user is what makes this work without administrator rights.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::Cryptography::{
    CertAddEncodedCertificateToStore, CertCloseStore, CertDeleteCertificateFromStore,
    CertFindCertificateInStore, CertFreeCertificateContext, CertOpenStore, CERT_CONTEXT,
    CERT_FIND_HASH, CERT_STORE_ADD_REPLACE_EXISTING, CERT_STORE_PROV_SYSTEM_W,
    CERT_SYSTEM_STORE_CURRENT_USER_ID, CERT_SYSTEM_STORE_LOCATION_SHIFT, CRYPT_INTEGER_BLOB,
    HCERTSTORE, X509_ASN_ENCODING,
};

use super::CertError;

// "Root", nul-terminated utf-16
const ROOT: &[u16] = &['R' as u16, 'o' as u16, 'o' as u16, 't' as u16, 0];

// per-user, not per-machine
const CURRENT_USER: u32 = CERT_SYSTEM_STORE_CURRENT_USER_ID << CERT_SYSTEM_STORE_LOCATION_SHIFT;

// raii around a store handle, so no early return leaks it
struct Store(HCERTSTORE);

impl Store {
    fn open() -> Result<Self, CertError> {
        // safety: ROOT is a valid nul-terminated wide string that outlives the call
        let handle = unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_W,
                0,
                0,
                CURRENT_USER,
                ROOT.as_ptr() as *const c_void,
            )
        };
        if handle.is_null() {
            return Err(CertError::StoreFailed(format!(
                "could not open the current user's Root store (error {})",
                last_error()
            )));
        }
        Ok(Store(handle))
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // safety: the handle came from CertOpenStore and is closed exactly once
        unsafe { CertCloseStore(self.0, 0) };
    }
}

fn last_error() -> u32 {
    // safety: no preconditions
    unsafe { GetLastError() }
}

pub fn add_to_current_user_root(der: &[u8]) -> Result<(), CertError> {
    let store = Store::open()?;

    // safety: der is valid for der.len() bytes. REPLACE_EXISTING makes re-adding
    // the same certificate a no-op rather than an error.
    let ok = unsafe {
        CertAddEncodedCertificateToStore(
            store.0,
            X509_ASN_ENCODING,
            der.as_ptr(),
            der.len() as u32,
            CERT_STORE_ADD_REPLACE_EXISTING,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(CertError::StoreFailed(format!(
            "the certificate was rejected by the store (error {})",
            last_error()
        )));
    }
    Ok(())
}

fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 || hex.is_empty() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

// the caller owns the returned context
fn find(store: &Store, thumbprint_hex: &str) -> Option<*const CERT_CONTEXT> {
    let mut bytes = hex_to_bytes(thumbprint_hex)?;
    let mut blob = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };

    // safety: blob and the buffer it points at are live for the whole call
    let found = unsafe {
        CertFindCertificateInStore(
            store.0,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_HASH,
            &mut blob as *mut _ as *const c_void,
            std::ptr::null(),
        )
    };
    (!found.is_null()).then_some(found as *const CERT_CONTEXT)
}

pub fn present(thumbprint_hex: &str) -> bool {
    let Ok(store) = Store::open() else {
        return false;
    };
    match find(&store, thumbprint_hex) {
        Some(ctx) => {
            // safety: ctx came from CertFindCertificateInStore and is freed once
            unsafe { CertFreeCertificateContext(ctx) };
            true
        }
        None => false,
    }
}

// Ok(false) means it was not there to begin with
pub fn remove(thumbprint_hex: &str) -> Result<bool, CertError> {
    let store = Store::open()?;
    let Some(ctx) = find(&store, thumbprint_hex) else {
        return Ok(false);
    };
    // safety: CertDeleteCertificateFromStore frees the context even on failure
    let ok = unsafe { CertDeleteCertificateFromStore(ctx) };
    if ok == 0 {
        return Err(CertError::StoreFailed(format!(
            "could not remove the certificate (error {})",
            last_error()
        )));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex_to_bytes("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert_eq!(hex_to_bytes("ABCD").unwrap(), vec![0xab, 0xcd]);
        assert!(hex_to_bytes("abc").is_none(), "odd length is not hex bytes");
        assert!(hex_to_bytes("zz").is_none(), "non-hex must be rejected");
        assert!(hex_to_bytes("").is_none(), "empty is not a thumbprint");
    }

    #[test]
    fn the_store_name_is_a_terminated_wide_string() {
        assert_eq!(
            ROOT.last(),
            Some(&0),
            "CertOpenStore needs a NUL terminator"
        );
        let name: String = ROOT[..ROOT.len() - 1]
            .iter()
            .map(|c| char::from_u32(*c as u32).unwrap())
            .collect();
        assert_eq!(name, "Root");
    }

    // a machine-wide store would need elevation, which this design avoids
    #[test]
    fn the_store_location_is_the_current_user() {
        assert_eq!(CURRENT_USER, 1 << 16);
        assert_eq!(CERT_SYSTEM_STORE_CURRENT_USER_ID, 1);
    }
}
