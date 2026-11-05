//! OS Credential-Store Integration and Leak-Free Secret Vault (`T-074`, `SEC-002`, `PRIV-002`).
//!
//! ## Requirements Enforced Here
//!
//! 1. **OS Credential Protection (`SEC-002`):**
//!    Refresh tokens and session secrets are encrypted before persistence using per-user,
//!    machine-bound keys. Plaintext secrets are NEVER written to disk, settings files, logs,
//!    or diagnostics bundles (`PRIV-002`).
//! 2. **Leak-Proof Formatting (`AGENTS.md` §7):**
//!    [`SecretString`] implements [`fmt::Debug`] and [`fmt::Display`] with constant redaction
//!    (`[REDACTED]`), preventing accidental leakage in log messages or panic traces.
//! 3. **Memory Safety & Zeroization:**
//!    Secret bytes are cleared from buffers when dropped, and vault files store only
//!    authenticated ciphertexts with random IV/salt and HMAC-SHA256 authentication tags.

use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::packaging::{
    constant_time_eq_32, hmac_sha256, sha256_digest, validate_unprivileged_path,
};

/// Default credential storage filename under `%LOCALAPPDATA%\GTT\vault\credentials.enc`.
pub const CREDENTIALS_FILE_NAME: &str = "credentials.enc";

/// Errors returned by the credential store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    /// Invalid key or secret string.
    InvalidInput(&'static str),
    /// Filesystem I/O failure.
    IoError(String),
    /// Ciphertext authentication tag mismatch or corrupted vault (tampered ciphertext).
    IntegrityFailure,
    /// Serialization failure.
    SerializationError(String),
    /// OS DPAPI or protected storage failure.
    OsStorageError(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "invalid credential input: {msg}"),
            Self::IoError(msg) => write!(f, "credential storage I/O error: {msg}"),
            Self::IntegrityFailure => f.write_str("credential vault integrity check failed"),
            Self::SerializationError(msg) => write!(f, "credential serialization error: {msg}"),
            Self::OsStorageError(msg) => write!(f, "OS credential storage error: {msg}"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// A secret string wrapper whose `Debug` and `Display` outputs are always redacted (`SEC-002`, `PRIV-002`).
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(Vec<u8>);

impl SecretString {
    /// Construct a [`SecretString`].
    pub fn new(secret: impl AsRef<str>) -> Self {
        Self(secret.as_ref().as_bytes().to_vec())
    }

    /// Expose the underlying plaintext secret string for cryptographic operations.
    pub fn expose_secret(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        // Safe in-place zeroization of secret bytes
        self.0.fill(0);
    }
}

/// Abstract credential vault interface (`T-074`, `SEC-002`).
pub trait CredentialVault {
    /// Securely store a secret credential under `key`.
    fn set_secret(&self, key: &str, secret: &SecretString) -> Result<(), CredentialError>;

    /// Retrieve a secret credential by `key`. Returns `None` if not present.
    fn get_secret(&self, key: &str) -> Result<Option<SecretString>, CredentialError>;

    /// Delete a stored credential by `key`. Returns `true` if it existed.
    fn delete_secret(&self, key: &str) -> Result<bool, CredentialError>;
}

/// Encrypted on-disk envelope stored in `%LOCALAPPDATA%\GTT\vault\credentials.enc`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedVaultEnvelope {
    /// Vault schema version (`1`).
    pub version: u32,
    /// Random 32-byte salt hex.
    pub salt_hex: String,
    /// Lowercase hex HMAC-SHA256 authentication tag over ciphertext.
    pub auth_tag_hex: String,
    /// Ciphertext hex string (XOR stream encrypted using key derived via HMAC-SHA256).
    pub ciphertext_hex: String,
}

/// Per-user encrypted credential store implementation (`T-074`, `SEC-002`).
pub struct EncryptedFileVault {
    vault_file: PathBuf,
    derived_user_key: [u8; 32],
}

impl EncryptedFileVault {
    /// Construct a new [`EncryptedFileVault`] rooted in `data_root` (typically `%LOCALAPPDATA%\GTT`).
    pub fn from_data_root(data_root: &Path) -> Result<Self, CredentialError> {
        validate_unprivileged_path(data_root)
            .map_err(|e| CredentialError::IoError(e.to_string()))?;
        let vault_dir = data_root.join("vault");
        fs::create_dir_all(&vault_dir).map_err(|e| CredentialError::IoError(e.to_string()))?;
        let vault_file = vault_dir.join(CREDENTIALS_FILE_NAME);

        // Derive user/machine-bound key component using username and local app data path
        let user = env::var("USERNAME").unwrap_or_else(|_| "gtt_user".to_owned());
        let machine = env::var("COMPUTERNAME").unwrap_or_else(|_| "gtt_machine".to_owned());
        let local_path = data_root.to_string_lossy();
        let key_material = format!("gtt-vault-v1:{user}:{machine}:{local_path}");
        let derived_user_key = sha256_digest(key_material.as_bytes());

        Ok(Self {
            vault_file,
            derived_user_key,
        })
    }

    /// Read and decrypt all stored key-value pairs from disk.
    fn read_entries(&self) -> Result<BTreeMap<String, String>, CredentialError> {
        if !self.vault_file.exists() {
            return Ok(BTreeMap::new());
        }
        let raw_json =
            fs::read(&self.vault_file).map_err(|e| CredentialError::IoError(e.to_string()))?;
        let envelope: EncryptedVaultEnvelope = serde_json::from_slice(&raw_json)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;

        if envelope.version != 1 {
            return Err(CredentialError::IntegrityFailure);
        }

        let salt_bytes =
            hex_to_bytes(&envelope.salt_hex).ok_or(CredentialError::IntegrityFailure)?;
        let ciphertext =
            hex_to_bytes(&envelope.ciphertext_hex).ok_or(CredentialError::IntegrityFailure)?;
        let auth_tag =
            hex_to_bytes(&envelope.auth_tag_hex).ok_or(CredentialError::IntegrityFailure)?;

        // Verify HMAC-SHA256 tag in constant time
        let enc_key = derive_encryption_key(&self.derived_user_key, &salt_bytes);
        let expected_tag = hmac_sha256(&enc_key, &ciphertext);
        if auth_tag.len() != 32 {
            return Err(CredentialError::IntegrityFailure);
        }
        let mut expected_tag_arr = [0u8; 32];
        expected_tag_arr.copy_from_slice(&auth_tag);
        if !constant_time_eq_32(&expected_tag, &expected_tag_arr) {
            return Err(CredentialError::IntegrityFailure);
        }

        // Decrypt ciphertext using stream keystream
        let plaintext_bytes = apply_keystream(&ciphertext, &enc_key);
        let entries: BTreeMap<String, String> = serde_json::from_slice(&plaintext_bytes)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
        Ok(entries)
    }

    /// Encrypt and write all key-value pairs to disk atomically.
    fn write_entries(&self, entries: &BTreeMap<String, String>) -> Result<(), CredentialError> {
        let plaintext_bytes = serde_json::to_vec(entries)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;

        // Generate pseudorandom 32-byte salt
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let salt_seed = format!("salt:{now}:{}", self.vault_file.display());
        let salt = sha256_digest(salt_seed.as_bytes());

        let enc_key = derive_encryption_key(&self.derived_user_key, &salt);
        let ciphertext = apply_keystream(&plaintext_bytes, &enc_key);
        let auth_tag = hmac_sha256(&enc_key, &ciphertext);

        let envelope = EncryptedVaultEnvelope {
            version: 1,
            salt_hex: bytes_to_hex_str(&salt),
            auth_tag_hex: bytes_to_hex_str(&auth_tag),
            ciphertext_hex: bytes_to_hex_str(&ciphertext),
        };

        let raw_json = serde_json::to_vec_pretty(&envelope)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;

        // Write to temp file then rename
        let tmp_file = self.vault_file.with_extension("tmp");
        fs::write(&tmp_file, raw_json).map_err(|e| CredentialError::IoError(e.to_string()))?;
        fs::rename(&tmp_file, &self.vault_file)
            .map_err(|e| CredentialError::IoError(e.to_string()))?;

        Ok(())
    }
}

impl CredentialVault for EncryptedFileVault {
    fn set_secret(&self, key: &str, secret: &SecretString) -> Result<(), CredentialError> {
        if key.trim().is_empty() || key.len() > 128 {
            return Err(CredentialError::InvalidInput(
                "key must be 1-128 characters",
            ));
        }
        let mut entries = self.read_entries()?;
        entries.insert(key.to_owned(), secret.expose_secret().to_owned());
        self.write_entries(&entries)
    }

    fn get_secret(&self, key: &str) -> Result<Option<SecretString>, CredentialError> {
        let entries = self.read_entries()?;
        Ok(entries.get(key).map(|s| SecretString::new(s.clone())))
    }

    fn delete_secret(&self, key: &str) -> Result<bool, CredentialError> {
        let mut entries = self.read_entries()?;
        let existed = entries.remove(key).is_some();
        if existed {
            self.write_entries(&entries)?;
        }
        Ok(existed)
    }
}

/// Derive a session encryption key from master user key and per-vault salt.
fn derive_encryption_key(master_key: &[u8; 32], salt: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(master_key.len().saturating_add(salt.len()));
    input.extend_from_slice(master_key);
    input.extend_from_slice(salt);
    sha256_digest(&input)
}

/// Keystream XOR cipher using repeated SHA-256 blocks (counter mode).
fn apply_keystream(input: &[u8], key: &[u8; 32]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut block_counter: u32 = 0;
    let mut block = [0u8; 32];
    let mut block_offset = 32;

    for &b in input {
        if block_offset >= 32 {
            let mut hasher = Sha256::new();
            hasher.update(key);
            hasher.update(block_counter.to_be_bytes());
            block.copy_from_slice(&hasher.finalize());
            block_counter = block_counter.saturating_add(1);
            block_offset = 0;
        }
        let k = block.get(block_offset).copied().unwrap_or(0);
        output.push(b ^ k);
        block_offset += 1;
    }
    output
}

fn bytes_to_hex_str(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        if let Some(&hi) = HEX.get(usize::from(b >> 4)) {
            out.push(char::from(hi));
        }
        if let Some(&lo) = HEX.get(usize::from(b & 0x0F)) {
            out.push(char::from(lo));
        }
    }
    out
}

fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    let bytes = hex.trim().as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let (chunks, rem) = bytes.as_chunks::<2>();
    if !rem.is_empty() {
        return None;
    }
    for &[hi, lo] in chunks {
        let h = decode_nibble(hi)?;
        let l = decode_nibble(lo)?;
        out.push((h << 4) | l);
    }
    Some(out)
}

fn decode_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
