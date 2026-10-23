// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! SNMPv3 user-based security model (RFC 3414, RFC 3826, RFC 7860):
//! localized keys, HMAC authentication, DES and AES privacy.

use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};

/// Authentication protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AuthProtocol {
    #[default]
    None,
    Md5,
    Sha,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

/// Privacy protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PrivProtocol {
    #[default]
    None,
    Des,
    Aes,
    /// AES-192 with the key extension from draft-blumenthal-aes-usm.
    Aes192,
    /// AES-256 with the key extension from draft-blumenthal-aes-usm.
    Aes256,
    /// AES-192 with the key extension used by Cisco (Reeder).
    Aes192C,
    /// AES-256 with the key extension used by Cisco (Reeder).
    Aes256C,
}

fn hash(p: AuthProtocol, data: &[u8]) -> Vec<u8> {
    match p {
        AuthProtocol::None => vec![],
        AuthProtocol::Md5 => Md5::digest(data).to_vec(),
        AuthProtocol::Sha => Sha1::digest(data).to_vec(),
        AuthProtocol::Sha224 => Sha224::digest(data).to_vec(),
        AuthProtocol::Sha256 => Sha256::digest(data).to_vec(),
        AuthProtocol::Sha384 => Sha384::digest(data).to_vec(),
        AuthProtocol::Sha512 => Sha512::digest(data).to_vec(),
    }
}

impl AuthProtocol {
    /// Length of the authentication parameters in messages.
    pub fn mac_len(self) -> usize {
        match self {
            AuthProtocol::None => 0,
            AuthProtocol::Md5 | AuthProtocol::Sha => 12,
            AuthProtocol::Sha224 => 16,
            AuthProtocol::Sha256 => 24,
            AuthProtocol::Sha384 => 32,
            AuthProtocol::Sha512 => 48,
        }
    }

    /// Password to key (RFC 3414 A.2.1): hash of the password repeated
    /// over 1 MiB.
    pub fn password_to_key(self, password: &[u8]) -> Vec<u8> {
        if password.is_empty() {
            return vec![];
        }
        let mut buf = Vec::with_capacity(1 << 20);
        while buf.len() < (1 << 20) {
            buf.extend_from_slice(password);
        }
        buf.truncate(1 << 20);
        hash(self, &buf)
    }

    /// Localize a key for an engine (RFC 3414 A.2.2).
    pub fn localize(self, key: &[u8], engine_id: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(key.len() * 2 + engine_id.len());
        buf.extend_from_slice(key);
        buf.extend_from_slice(engine_id);
        buf.extend_from_slice(key);
        hash(self, &buf)
    }

    /// Localized key from a password.
    pub fn localized_key(self, password: &[u8], engine_id: &[u8]) -> Vec<u8> {
        self.localize(&self.password_to_key(password), engine_id)
    }

    /// Compute the truncated HMAC of a message.
    pub fn mac(self, key: &[u8], msg: &[u8]) -> Vec<u8> {
        macro_rules! h {
            ($t:ty) => {{
                let mut m = <Hmac<$t>>::new_from_slice(key).expect("HMAC accepts any key length");
                m.update(msg);
                m.finalize().into_bytes().to_vec()
            }};
        }
        let full = match self {
            AuthProtocol::None => return vec![],
            AuthProtocol::Md5 => h!(Md5),
            AuthProtocol::Sha => h!(Sha1),
            AuthProtocol::Sha224 => h!(Sha224),
            AuthProtocol::Sha256 => h!(Sha256),
            AuthProtocol::Sha384 => h!(Sha384),
            AuthProtocol::Sha512 => h!(Sha512),
        };
        full[..self.mac_len()].to_vec()
    }
}

impl PrivProtocol {
    fn key_len(self) -> usize {
        match self {
            PrivProtocol::None => 0,
            PrivProtocol::Des | PrivProtocol::Aes => 16,
            PrivProtocol::Aes192 | PrivProtocol::Aes192C => 24,
            PrivProtocol::Aes256 | PrivProtocol::Aes256C => 32,
        }
    }

    /// Localized privacy key, extended if needed.
    pub fn localized_key(self, auth: AuthProtocol, password: &[u8], engine_id: &[u8]) -> Vec<u8> {
        if self == PrivProtocol::None {
            return vec![];
        }
        let mut key = auth.localized_key(password, engine_id);
        let needed = self.key_len();
        match self {
            PrivProtocol::Aes192 | PrivProtocol::Aes256 => {
                // Blumenthal: append the hash of the key so far.
                while key.len() < needed {
                    let h = hash(auth, &key);
                    key.extend_from_slice(&h);
                }
            }
            PrivProtocol::Aes192C | PrivProtocol::Aes256C => {
                // Reeder: append the localized key derived from the key.
                while key.len() < needed {
                    let more = auth.localized_key(&key, engine_id);
                    key.extend_from_slice(&more);
                }
            }
            _ => {}
        }
        key.truncate(needed);
        key
    }

    /// Encrypt a scoped PDU. Returns the ciphertext and the privacy
    /// parameters (salt).
    pub fn encrypt(
        self,
        key: &[u8],
        engine_boots: u32,
        engine_time: u32,
        salt: u64,
        data: &[u8],
    ) -> (Vec<u8>, Vec<u8>) {
        match self {
            PrivProtocol::None => (data.to_vec(), vec![]),
            PrivProtocol::Des => {
                use cbc::cipher::{BlockModeEncrypt, KeyIvInit};
                let mut salt_bytes = Vec::with_capacity(8);
                salt_bytes.extend_from_slice(&engine_boots.to_be_bytes());
                salt_bytes.extend_from_slice(&(salt as u32).to_be_bytes());
                let iv: Vec<u8> = key[8..16].iter().zip(&salt_bytes).map(|(a, b)| a ^ b).collect();
                let mut buf = data.to_vec();
                // Pad to a multiple of 8 bytes.
                buf.resize(data.len().div_ceil(8) * 8, 0);
                let n = buf.len();
                let enc = cbc::Encryptor::<des::Des>::new_from_slices(&key[..8], &iv).unwrap();
                enc.encrypt_padded::<cbc::cipher::block_padding::NoPadding>(&mut buf, n).unwrap();
                (buf, salt_bytes)
            }
            _ => {
                let salt_bytes = salt.to_be_bytes().to_vec();
                let mut iv = Vec::with_capacity(16);
                iv.extend_from_slice(&engine_boots.to_be_bytes());
                iv.extend_from_slice(&engine_time.to_be_bytes());
                iv.extend_from_slice(&salt_bytes);
                let mut buf = data.to_vec();
                aes_cfb(key, &iv, &mut buf, true);
                (buf, salt_bytes)
            }
        }
    }

    /// Decrypt a scoped PDU.
    pub fn decrypt(
        self,
        key: &[u8],
        engine_boots: u32,
        engine_time: u32,
        params: &[u8],
        data: &[u8],
    ) -> Result<Vec<u8>, String> {
        match self {
            PrivProtocol::None => Ok(data.to_vec()),
            PrivProtocol::Des => {
                use cbc::cipher::{BlockModeDecrypt, KeyIvInit};
                if params.len() != 8 || !data.len().is_multiple_of(8) {
                    return Err("invalid DES parameters".into());
                }
                let iv: Vec<u8> = key[8..16].iter().zip(params).map(|(a, b)| a ^ b).collect();
                let mut buf = data.to_vec();
                let dec = cbc::Decryptor::<des::Des>::new_from_slices(&key[..8], &iv).unwrap();
                dec.decrypt_padded::<cbc::cipher::block_padding::NoPadding>(&mut buf).map_err(|e| e.to_string())?;
                Ok(buf)
            }
            _ => {
                if params.len() != 8 {
                    return Err("invalid AES parameters".into());
                }
                let mut iv = Vec::with_capacity(16);
                iv.extend_from_slice(&engine_boots.to_be_bytes());
                iv.extend_from_slice(&engine_time.to_be_bytes());
                iv.extend_from_slice(params);
                let mut buf = data.to_vec();
                aes_cfb(key, &iv, &mut buf, false);
                Ok(buf)
            }
        }
    }
}

fn aes_cfb(key: &[u8], iv: &[u8], buf: &mut [u8], encrypt: bool) {
    use cfb_mode::cipher::KeyIvInit;
    macro_rules! run {
        ($aes:ty) => {{
            if encrypt {
                cfb_mode::Encryptor::<$aes>::new_from_slices(key, iv).unwrap().encrypt(buf)
            } else {
                cfb_mode::Decryptor::<$aes>::new_from_slices(key, iv).unwrap().decrypt(buf)
            }
        }};
    }
    match key.len() {
        16 => run!(aes::Aes128),
        24 => run!(aes::Aes192),
        _ => run!(aes::Aes256),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn rfc3414_keys() {
        // RFC 3414 A.3.1 and A.3.2: password "maplesyrup", engine ID
        // 000000000000000000000002.
        let engine = [0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];
        let ku = AuthProtocol::Md5.password_to_key(b"maplesyrup");
        assert_eq!(hex(&ku), "9faf3283884e92834ebc9847d8edd963");
        assert_eq!(hex(&AuthProtocol::Md5.localize(&ku, &engine)), "526f5eed9fcce26f8964c2930787d82b");
        let ku = AuthProtocol::Sha.password_to_key(b"maplesyrup");
        assert_eq!(hex(&ku), "9fb5cc0381497b3793528939ff788d5d79145211");
        assert_eq!(hex(&AuthProtocol::Sha.localize(&ku, &engine)), "6695febc9288e36282235fc7151f128497b38f3f");
    }

    #[test]
    fn privacy_roundtrip() {
        let engine = b"\x80\x00\x1f\x88\x80\x01\x02\x03\x04";
        for p in
            [PrivProtocol::Des, PrivProtocol::Aes, PrivProtocol::Aes192, PrivProtocol::Aes256, PrivProtocol::Aes256C]
        {
            let key = p.localized_key(AuthProtocol::Sha, b"secret-passphrase", engine);
            let data = b"some scoped PDU data, not a multiple of 8".to_vec();
            let (enc, params) = p.encrypt(&key, 3, 1000, 42, &data);
            let dec = p.decrypt(&key, 3, 1000, &params, &enc).unwrap();
            assert_eq!(&dec[..data.len()], &data[..], "{p:?}");
        }
    }
}
