//! Token encryption at rest: DPAPI, current-user scope.

#[cfg(windows)]
pub fn protect(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&input, None, None, None, None, 0, &mut out)?;
        Ok(take_blob(out))
    }
}

#[cfg(windows)]
pub fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: cipher.len() as u32, pbData: cipher.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut out).ok()?;
        Some(take_blob(out))
    }
}

#[cfg(windows)]
unsafe fn take_blob(blob: windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
    let _ = windows::Win32::Foundation::LocalFree(windows::Win32::Foundation::HLOCAL(blob.pbData as _));
    v
}

// The app only runs on Windows; this keeps the portable modules testable on Linux.
#[cfg(not(windows))]
pub fn protect(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(plain.to_vec())
}

#[cfg(not(windows))]
pub fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
    Some(cipher.to_vec())
}
