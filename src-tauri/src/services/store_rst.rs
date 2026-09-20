// SPDX-License-Identifier: GPL-3.0-only
//
// RST/CLEP pure algorithm helpers adapted from Xodus commit
// 0670e25aeb0e0e9f800f8f2f4968ae3b681842a7 and LeviLauncher nativeinstall,
// with CLEP derivation credited by Xodus to LukeFZ (MIT-licensed SPLicense work).
// Network transport, XML signatures, and Windows DPAPI deliberately remain
// outside this module; DPAPI is provided by store_device.

use openssl::{bn::BigNum, rsa::Rsa, symm::{Cipher, Crypter, Mode}};
use sha2::{Digest, Sha256};

const MAX_CLEP_SIZE: usize = 4096;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StoreRstError {
    #[error("invalid CLEP schedule length")]
    InvalidSchedule,
    #[error("invalid CBC input")]
    InvalidCbcInput,
    #[error("unsupported CLEP state")]
    UnsupportedClep,
    #[error("invalid device wrapping key")]
    InvalidDeviceKey,
    #[error("device wrapping key check failed")]
    DeviceKeyCheckFailed,
    #[error("invalid BCrypt RSA private blob")]
    InvalidRsaBlob,
    #[error("invalid RSA field")]
    InvalidRsaField,
    #[error("RSA key construction failed")]
    RsaConstruction,
    #[error("cryptographic operation failed")]
    Crypto,
    /// Device ticket acquisition is a Windows-only capability (the credential
    /// store lives behind DPAPI in `store_device` / `store_wam`).
    #[error("device ticket acquisition is only supported on Windows")]
    WindowsOnly,
    /// RST exchange failure carrying the transport layer's own diagnosis.
    #[error("device ticket exchange failed: {0}")]
    Exchange(String),
}

fn word(input: &[u8], index: usize) -> Result<u32, StoreRstError> {
    let start = index.checked_mul(4).ok_or(StoreRstError::InvalidSchedule)?;
    let end = start.checked_add(4).ok_or(StoreRstError::InvalidSchedule)?;
    let bytes = input.get(start..end).ok_or(StoreRstError::InvalidSchedule)?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| StoreRstError::InvalidSchedule)?))
}

/// Derive the 128-bit AES schedule key used by CLEP and device wrapping keys.
pub fn schedule_key(schedule: &[u8]) -> Result<[u8; 16], StoreRstError> {
    if schedule.len() < 232 { return Err(StoreRstError::InvalidSchedule); }
    let words = [
        word(schedule, 46)? ^ word(schedule, 56)? ^ 0xe20d_f371 ^ 0xccb2_2fe6,
        word(schedule, 36)? ^ word(schedule, 47)? ^ 0xdf08_0e39,
        word(schedule, 40)? ^ word(schedule, 51)? ^ 0x6d09_b2f5 ^ 0x2ae1_7ab9,
        word(schedule, 30)? ^ word(schedule, 41)? ^ 0x3728_8cec,
    ];
    let mut result = [0u8; 16];
    for (index, value) in words.iter().enumerate() { result[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes()); }
    Ok(result)
}

/// AES-128-CBC decrypt without implicit padding (matching Go's cipher.CBC).
pub fn cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, StoreRstError> {
    if key.len() != 16 || iv.len() != 16 || data.is_empty() || !data.len().is_multiple_of(16) { return Err(StoreRstError::InvalidCbcInput); }
    let mut crypter = Crypter::new(Cipher::aes_128_cbc(), Mode::Decrypt, key, Some(iv)).map_err(|_| StoreRstError::Crypto)?;
    crypter.pad(false);
    let mut output = vec![0u8; data.len().checked_add(16).ok_or(StoreRstError::InvalidCbcInput)?];
    let written = crypter.update(data, &mut output).map_err(|_| StoreRstError::Crypto)?;
    let final_len = crypter.finalize(&mut output[written..]).map_err(|_| StoreRstError::Crypto)?;
    output.truncate(written + final_len);
    Ok(output)
}

/// Decrypt the encrypted payload of a 4096-byte CLEP record.
pub fn decrypt_clep(blob: &[u8], encrypted_len: usize) -> Result<Vec<u8>, StoreRstError> {
    if blob.len() != MAX_CLEP_SIZE || blob.get(0..4).and_then(|bytes| bytes.try_into().ok()).map(u32::from_le_bytes) != Some(4) || encrypted_len == 0 || !encrypted_len.is_multiple_of(16) || encrypted_len > 4092 { return Err(StoreRstError::UnsupportedClep); }
    let encrypted_end = 4usize.checked_add(encrypted_len).ok_or(StoreRstError::UnsupportedClep)?;
    if encrypted_end > blob.len() { return Err(StoreRstError::UnsupportedClep); }
    let key = schedule_key(blob.get(encrypted_end..).ok_or(StoreRstError::UnsupportedClep)?)?;
    cbc_decrypt(&key, &[0u8; 16], blob.get(4..encrypted_end).ok_or(StoreRstError::UnsupportedClep)?)
}

/// Validate and extract the device wrapping key from a 4096-byte record.
pub fn device_wrapping_key(blob: &[u8]) -> Result<[u8; 16], StoreRstError> {
    if blob.len() != MAX_CLEP_SIZE || blob.get(0..2).and_then(|bytes| bytes.try_into().ok()).map(u16::from_le_bytes) != Some(4096) || blob.get(2..6).and_then(|bytes| bytes.try_into().ok()).map(u32::from_le_bytes) != Some(4) { return Err(StoreRstError::InvalidDeviceKey); }
    let key = schedule_key(blob.get(6..).ok_or(StoreRstError::InvalidDeviceKey)?)?;
    let check = cbc_decrypt(&key, &[0u8; 16], blob.get(518..534).ok_or(StoreRstError::InvalidDeviceKey)?)?;
    if check.as_slice() != key { return Err(StoreRstError::DeviceKeyCheckFailed); }
    Ok(key)
}

/// Parse a BCrypt `RSA2` private blob into an OpenSSL RSA private key.
pub fn parse_bcrypt_rsa_private(blob: &[u8]) -> Result<Rsa<openssl::pkey::Private>, StoreRstError> {
    if blob.len() < 24 || &blob[0..4] != b"RSA2" { return Err(StoreRstError::InvalidRsaBlob); }
    let read_u32 = |offset: usize| -> Result<usize, StoreRstError> {
        let end = offset.checked_add(4).ok_or(StoreRstError::InvalidRsaBlob)?;
        let bytes = blob.get(offset..end).ok_or(StoreRstError::InvalidRsaBlob)?;
        usize::try_from(u32::from_le_bytes(bytes.try_into().map_err(|_| StoreRstError::InvalidRsaBlob)?)).map_err(|_| StoreRstError::InvalidRsaBlob)
    };
    let mut position = 24usize;
    let mut read = |length: usize| -> Result<BigNum, StoreRstError> {
        if length == 0 || length > blob.len().saturating_sub(position) { return Err(StoreRstError::InvalidRsaField); }
        let value = BigNum::from_slice(&blob[position..position + length]).map_err(|_| StoreRstError::InvalidRsaField)?;
        position += length;
        Ok(value)
    };
    let e = read(read_u32(8)?)?;
    let n = read(read_u32(12)?)?;
    let p = read(read_u32(16)?)?;
    let q = read(read_u32(20)?)?;
    let one = BigNum::from_u32(1).map_err(|_| StoreRstError::RsaConstruction)?;
    let mut pm1 = BigNum::new().map_err(|_| StoreRstError::RsaConstruction)?; pm1.checked_sub(&p, &one).map_err(|_| StoreRstError::RsaConstruction)?;
    let mut qm1 = BigNum::new().map_err(|_| StoreRstError::RsaConstruction)?; qm1.checked_sub(&q, &one).map_err(|_| StoreRstError::RsaConstruction)?;
    let mut phi = BigNum::new().map_err(|_| StoreRstError::RsaConstruction)?; let mut mul_ctx = openssl::bn::BigNumContext::new().map_err(|_| StoreRstError::RsaConstruction)?;
    phi.checked_mul(&pm1, &qm1, &mut mul_ctx).map_err(|_| StoreRstError::RsaConstruction)?;
    let mut ctx = openssl::bn::BigNumContext::new().map_err(|_| StoreRstError::RsaConstruction)?;
    let mut d = BigNum::new().map_err(|_| StoreRstError::RsaConstruction)?;
    d.mod_inverse(&e, &phi, &mut ctx).map_err(|_| StoreRstError::RsaConstruction)?;
    let mut dmp1 = BigNum::new().map_err(|_| StoreRstError::RsaConstruction)?;
    dmp1.nnmod(&d, &pm1, &mut ctx).map_err(|_| StoreRstError::RsaConstruction)?;
    let mut dmq1 = BigNum::new().map_err(|_| StoreRstError::RsaConstruction)?;
    dmq1.nnmod(&d, &qm1, &mut ctx).map_err(|_| StoreRstError::RsaConstruction)?;
    let mut iqmp = BigNum::new().map_err(|_| StoreRstError::RsaConstruction)?;
    iqmp.mod_inverse(&q, &p, &mut ctx).map_err(|_| StoreRstError::RsaConstruction)?;
    Rsa::from_private_components(n, e, d, p, q, dmp1, dmq1, iqmp).map_err(|_| StoreRstError::RsaConstruction)
}

/// WS-SecureConversation double-derived HMAC-SHA256 key.
pub fn derived_key(key: &[u8], nonce: &[u8]) -> Result<[u8; 32], StoreRstError> {
    if key.is_empty() { return Err(StoreRstError::Crypto); }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    if key.len() > 64 {
        let digest = Sha256::digest(key);
        for (slot, value) in ipad.iter_mut().zip(digest.iter().copied().chain(std::iter::repeat(0))) { *slot ^= value; }
        for (slot, value) in opad.iter_mut().zip(digest.iter().copied().chain(std::iter::repeat(0))) { *slot ^= value; }
    } else {
        for (slot, value) in ipad.iter_mut().zip(key.iter().copied().chain(std::iter::repeat(0))) { *slot ^= value; }
        for (slot, value) in opad.iter_mut().zip(key.iter().copied().chain(std::iter::repeat(0))) { *slot ^= value; }
    }
    let mut inner = Sha256::new(); inner.update(ipad); inner.update([0, 0, 0, 1]); inner.update(b"WS-SecureConversationWS-SecureConversation"); inner.update([0]); inner.update(nonce); inner.update([0, 0, 1, 0]);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new(); outer.update(opad); outer.update(inner_digest);
    Ok(outer.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_rejects_short_input() { assert_eq!(schedule_key(&[0; 231]), Err(StoreRstError::InvalidSchedule)); }
    #[test]
    fn derived_is_deterministic() { assert_eq!(derived_key(b"secret", b"nonce").unwrap(), derived_key(b"secret", b"nonce").unwrap()); }
}
