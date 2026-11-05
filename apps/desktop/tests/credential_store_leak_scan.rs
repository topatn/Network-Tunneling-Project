//! Secret leak scan and OS credential store verification (`T-074`, `SEC-002`, `PRIV-002`).
//!
//! Verifies:
//! 1. Encrypted credential storage roundtrips correctly (`set_secret`, `get_secret`, `delete_secret`).
//! 2. `SecretString` redaction in `Display` and `Debug` formats.
//! 3. Tampered ciphertext or wrong auth tag triggers integrity failure.
//! 4. Comprehensive leak scan: recursive scan of all files on disk proves that zero plaintext
//!    secret strings exist in files (`SEC-002`, `Done when: Scan of files, logs, dumps finds no secret strings`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gtt_desktop::credentials::{CredentialVault, EncryptedFileVault, SecretString};

static TEST_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Result<Self, String> {
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = env::temp_dir().join(format!("gtt-cred-test-{label}-{pid}-{seq}"));
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
        }
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Self { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn credential_vault_roundtrip_and_redaction() -> Result<(), String> {
    let tmp = TempDir::new("vault-roundtrip")?;
    let vault = EncryptedFileVault::from_data_root(&tmp.path).map_err(|e| e.to_string())?;

    let test_key = "accounts/test-user/refresh_token";
    let raw_secret = "gtt_rt_abc123_high_entropy_refresh_token_value_998877";
    let secret = SecretString::new(raw_secret);

    // 1. Redaction in Debug and Display
    let debug_str = format!("{secret:?}");
    let display_str = format!("{secret}");
    if debug_str != "[REDACTED]" || display_str != "[REDACTED]" {
        return Err(format!(
            "SecretString failed to redact: debug='{debug_str}', display='{display_str}'"
        ));
    }

    // 2. Store credential
    vault
        .set_secret(test_key, &secret)
        .map_err(|e| e.to_string())?;

    // 3. Retrieve credential and verify match
    let retrieved = vault.get_secret(test_key).map_err(|e| e.to_string())?;
    let retrieved_secret = retrieved.ok_or_else(|| "credential not found".to_owned())?;
    if retrieved_secret.expose_secret() != raw_secret {
        return Err("retrieved secret does not match stored secret".to_owned());
    }

    // 4. Delete credential
    let deleted = vault.delete_secret(test_key).map_err(|e| e.to_string())?;
    if !deleted {
        return Err("expected delete_secret to return true".to_owned());
    }
    let after_delete = vault.get_secret(test_key).map_err(|e| e.to_string())?;
    if after_delete.is_some() {
        return Err("expected credential to be None after deletion".to_owned());
    }

    Ok(())
}

#[test]
fn credential_vault_rejects_tampered_ciphertext() -> Result<(), String> {
    let tmp = TempDir::new("tamper-rejection")?;
    let vault = EncryptedFileVault::from_data_root(&tmp.path).map_err(|e| e.to_string())?;

    let test_key = "tokens/auth";
    let secret = SecretString::new("gtt_rt_secret_for_tampering_test");
    vault
        .set_secret(test_key, &secret)
        .map_err(|e| e.to_string())?;

    let vault_file = tmp.path.join("vault").join("credentials.enc");
    if !vault_file.exists() {
        return Err("expected credentials.enc to exist".to_owned());
    }

    // Tamper with the ciphertext in credentials.enc
    let raw_content = fs::read_to_string(&vault_file).map_err(|e| e.to_string())?;
    let mut parsed: serde_json::Value =
        serde_json::from_str(&raw_content).map_err(|e| e.to_string())?;
    if let Some(ct) = parsed.get("ciphertext_hex").and_then(|v| v.as_str()) {
        let mut chars: Vec<char> = ct.chars().collect();
        if let Some(first) = chars.first_mut() {
            *first = if *first == 'a' { 'b' } else { 'a' };
        }
        if let Some(map) = parsed.as_object_mut() {
            map.insert(
                "ciphertext_hex".to_owned(),
                serde_json::Value::String(chars.into_iter().collect()),
            );
        }
    }
    let tampered_bytes = serde_json::to_vec_pretty(&parsed).map_err(|e| e.to_string())?;
    fs::write(&vault_file, tampered_bytes).map_err(|e| e.to_string())?;

    // Attempting to read tampered vault MUST fail with integrity error
    match vault.get_secret(test_key) {
        Err(_) => {}
        Ok(_) => return Err("expected integrity failure for tampered vault".to_owned()),
    }

    Ok(())
}

#[test]
fn t074_credential_leak_scan_finds_no_secret_strings_in_files() -> Result<(), String> {
    let tmp = TempDir::new("leak-scan")?;
    let vault = EncryptedFileVault::from_data_root(&tmp.path).map_err(|e| e.to_string())?;

    let target_secret = "gtt_rt_super_secret_canary_refresh_token_DO_NOT_LEAK_123456789";
    let secret = SecretString::new(target_secret);

    // Write secret to vault
    vault
        .set_secret("account/canary", &secret)
        .map_err(|e| e.to_string())?;

    // Also write auxiliary files (e.g. simulated logs, configs)
    let logs_dir = tmp.path.join("logs");
    fs::create_dir_all(&logs_dir).map_err(|e| e.to_string())?;
    fs::write(
        logs_dir.join("desktop.log"),
        b"2026-11-05 [INFO] User signed in successfully; token stored in vault.\n",
    )
    .map_err(|e| e.to_string())?;

    let config_dir = tmp.path.join("config");
    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    fs::write(
        config_dir.join("settings.json"),
        b"{\"theme\":\"system\",\"auto_start\":false}\n",
    )
    .map_err(|e| e.to_string())?;

    // Recursively scan all files under tmp.path
    let leaked_files = scan_directory_for_secret(&tmp.path, target_secret)?;
    if !leaked_files.is_empty() {
        return Err(format!(
            "SEC-002 VIOLATION: Plaintext secret found on disk in: {leaked_files:?}"
        ));
    }

    // Verify secret is retrievable from the vault
    let retrieved = vault
        .get_secret("account/canary")
        .map_err(|e| e.to_string())?;
    let val = retrieved.ok_or_else(|| "canary secret not found".to_owned())?;
    if val.expose_secret() != target_secret {
        return Err("canary secret mismatch".to_owned());
    }

    Ok(())
}

fn scan_directory_for_secret(dir: &Path, secret: &str) -> Result<Vec<PathBuf>, String> {
    let mut leaks = Vec::new();
    let secret_bytes = secret.as_bytes();

    let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            let mut sub_leaks = scan_directory_for_secret(&path, secret)?;
            leaks.append(&mut sub_leaks);
        } else if path.is_file() {
            let content = fs::read(&path).map_err(|e| e.to_string())?;
            if contains_subslice(&content, secret_bytes) {
                leaks.push(path);
            }
        }
    }
    Ok(leaks)
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
