// Modified for NoriShell; see vendor/README.md at the repository root for upstream provenance.
use aes::*;
use ssh_key::PrivateKey;

#[cfg(any(feature = "des", feature = "legacy-pem-3des"))]
use des::TdesEde3;

use super::Encryption;
use crate::keys::Error;

/// Decode a secret key in the PKCS#5 format, possibly deciphering it
/// using the supplied password.
pub fn decode_pkcs5(
    secret: &[u8],
    password: Option<&str>,
    enc: Encryption,
) -> Result<PrivateKey, Error> {
    use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};

    if let Some(pass) = password {
        let sec = match enc {
            Encryption::Aes128Cbc(ref iv) => {
                let key = legacy_pem_key(pass.as_bytes(), &iv[..8], 16);

                #[allow(clippy::unwrap_used)] // AES parameters are static
                let c = cbc::Decryptor::<Aes128>::new_from_slices(&key, &iv[..]).unwrap();
                let mut dec = secret.to_vec();
                c.decrypt_padded::<Pkcs7>(&mut dec)?.to_vec()
            }
            Encryption::Aes256Cbc(ref iv) => {
                let key = legacy_pem_key(pass.as_bytes(), &iv[..8], 32);

                #[allow(clippy::unwrap_used)] // AES parameters are static
                let c = cbc::Decryptor::<Aes256>::new_from_slices(&key, &iv[..]).unwrap();
                let mut dec = secret.to_vec();
                c.decrypt_padded::<Pkcs7>(&mut dec)?.to_vec()
            }
            Encryption::TripleDesCbc(ref iv) => {
                #[cfg(any(feature = "des", feature = "legacy-pem-3des"))]
                {
                    let key = legacy_pem_key(pass.as_bytes(), iv, 24);
                    #[allow(clippy::unwrap_used)] // 3DES parameters are static
                    let c = cbc::Decryptor::<TdesEde3>::new_from_slices(&key, iv).unwrap();
                    let mut dec = secret.to_vec();
                    c.decrypt_padded::<Pkcs7>(&mut dec)?.to_vec()
                }
                #[cfg(not(any(feature = "des", feature = "legacy-pem-3des")))]
                {
                    return Err(Error::UnsupportedKeyType {
                        key_type_string: "DES-EDE3-CBC".to_string(),
                        key_type_raw: vec![],
                    });
                }
            }
        };
        // TODO: presumably pkcs5 could contain non-RSA keys?
        #[cfg(feature = "rsa")]
        {
            super::decode_rsa_pkcs1_der(&sec).map(Into::into)
        }
        #[cfg(not(feature = "rsa"))]
        {
            Err(Error::UnsupportedKeyType {
                key_type_string: "RSA".to_string(),
                key_type_raw: vec![],
            })
        }
    } else {
        Err(Error::KeyIsEncrypted)
    }
}

fn legacy_pem_key(password: &[u8], salt: &[u8], length: usize) -> Vec<u8> {
    let mut key = Vec::with_capacity(length);
    let mut previous = Vec::new();
    while key.len() < length {
        let mut digest = md5::Context::new();
        if !previous.is_empty() {
            digest.consume(&previous);
        }
        digest.consume(password);
        digest.consume(salt);
        previous = digest.finalize().0.to_vec();
        key.extend_from_slice(&previous);
    }
    key.truncate(length);
    key
}

#[cfg(test)]
mod tests {
    use aes::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
    use data_encoding::{BASE64_MIME, HEXUPPER};
    use pkcs1::EncodeRsaPrivateKey;
    use rsa::RsaPrivateKey;

    use super::{Aes256, legacy_pem_key};
    #[cfg(any(feature = "des", feature = "legacy-pem-3des"))]
    use super::TdesEde3;
    use crate::keys::format::decode_secret_key;

    const PASSPHRASE: &str = "norishell-legacy-pem";

    fn test_rsa_der() -> Vec<u8> {
        let key = RsaPrivateKey::new(&mut rand::rng(), 1024).expect("generate test RSA key");
        key.to_pkcs1_der()
            .expect("encode RSA key")
            .as_bytes()
            .to_vec()
    }

    fn as_legacy_pem(algorithm: &str, iv: &[u8], encrypted: &[u8]) -> String {
        format!(
            "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: {algorithm},{}\n\n{}\n-----END RSA PRIVATE KEY-----\n",
            HEXUPPER.encode(iv),
            BASE64_MIME.encode(encrypted),
        )
    }

    #[test]
    fn decodes_aes256_cbc_legacy_pem() {
        let der = test_rsa_der();
        let iv = [0x42; 16];
        let key = legacy_pem_key(PASSPHRASE.as_bytes(), &iv[..8], 32);
        #[allow(clippy::unwrap_used)] // test parameters are static
        let cipher = cbc::Encryptor::<Aes256>::new_from_slices(&key, &iv).unwrap();
        let mut padded = der;
        let message_len = padded.len();
        padded.resize(message_len + 16, 0);
        let encrypted = cipher
            .encrypt_padded::<Pkcs7>(&mut padded, message_len)
            .expect("encrypt test key")
            .to_vec();
        let pem = as_legacy_pem("AES-256-CBC", &iv, &encrypted);

        assert!(decode_secret_key(&pem, Some(PASSPHRASE))
            .expect("decode AES-256 PEM")
            .algorithm()
            .is_rsa());
        assert!(decode_secret_key(&pem, Some("wrong-passphrase")).is_err());
    }

    #[cfg(any(feature = "des", feature = "legacy-pem-3des"))]
    #[test]
    fn decodes_3des_cbc_legacy_pem() {
        let der = test_rsa_der();
        let iv = [0x24; 8];
        let key = legacy_pem_key(PASSPHRASE.as_bytes(), &iv, 24);
        #[allow(clippy::unwrap_used)] // test parameters are static
        let cipher = cbc::Encryptor::<TdesEde3>::new_from_slices(&key, &iv).unwrap();
        let mut padded = der;
        let message_len = padded.len();
        padded.resize(message_len + 8, 0);
        let encrypted = cipher
            .encrypt_padded::<Pkcs7>(&mut padded, message_len)
            .expect("encrypt test key")
            .to_vec();
        let pem = as_legacy_pem("DES-EDE3-CBC", &iv, &encrypted);

        assert!(decode_secret_key(&pem, Some(PASSPHRASE))
            .expect("decode 3DES PEM")
            .algorithm()
            .is_rsa());
        assert!(decode_secret_key(&pem, Some("wrong-passphrase")).is_err());
    }
}
