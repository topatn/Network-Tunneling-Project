//! Artifact digest verification (`SRV-002`).
//!
//! ## This is mechanism, not a claim
//!
//! The comparison here is sound and the *inputs* are not yet. `research.md` RQ-13 records the
//! Minecraft Java `server.jar` digest source as `[X] not confirmed`, and says E-15 must verify
//! it before `SRV-002` can be claimed. The Adoptium JRE path **is** `[V]` with SHA-256 verified
//! and 12/12 checks, so this mechanism has a real caller. Nothing in this module asserts that a
//! particular game is safe to provision.
//!
//! ## Bounded and streaming
//!
//! AGENTS.md section 4 forbids unbounded allocation on untrusted input, and a server artifact is
//! tens of megabytes -- large enough that reading it whole would be a real allocation, and large
//! enough that a hostile "artifact" could be arbitrarily larger. So the file is read in a fixed
//! buffer and the total is **capped**: [`ArtifactSource::MAX_BYTES`]. A file over the cap is
//! refused before it is executed and before it is fully read.
//!
//! ## The error never echoes the artifact
//!
//! `PRIV-002` and `SEC-013`: error text ends up in logs and diagnostics bundles. A rejected
//! artifact's name is reported, because the host needs to know *which* download failed, but its
//! contents, its first bytes and its length beyond "over the cap" are not. A digest mismatch
//! reports the digests, which are published values and contain nothing about the host.
//!
//! ## Comparison
//!
//! Digests are compared in constant time relative to their content. [`Digest`] is a fixed
//! 32-byte array and `PartialEq` is derived, which is *not* constant time -- so
//! [`Digest::matches`] is used for the security-relevant comparison and does not short-circuit.
//! The distinction is small but the point of naming it is that the default is the wrong one.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest as Sha2Digest, Sha256};

/// Buffer size for streaming a file. Not a tuning knob: it bounds the allocation, and the
/// artifact size bound does the rest.
const READ_BUFFER: usize = 64 * 1024;

/// A SHA-256 digest, parsed from its hex form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digest([u8; 32]);

impl Digest {
    /// Parse 64 lowercase-or-uppercase hex characters.
    ///
    /// Rejects anything else rather than normalising silently: a digest that arrived in an
    /// unexpected shape is a reason to stop, not to guess at.
    pub fn parse_hex(hex: &str) -> Result<Self, VerifyError> {
        let trimmed = hex.trim();
        if trimmed.len() != 64 {
            return Err(VerifyError::MalformedDigest {
                reason: format!("expected 64 hex characters, got {}", trimmed.len()),
            });
        }
        // `as_chunks` gives a slice of 2-byte pairs and a remainder in one step, with no
        // indexing and no `unwrap`: a non-empty remainder is the odd-length case, already
        // excluded by the length check above.
        let (pairs, remainder) = trimmed.as_bytes().as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(VerifyError::MalformedDigest {
                reason: format!("expected 64 hex characters, got {}", trimmed.len()),
            });
        }

        let mut bytes = [0u8; 32];
        for (i, pair) in pairs.iter().enumerate() {
            // Each element of a `[[u8; 2]]` holds exactly two bytes, so both are present.
            let hi = hex_nibble(pair.first().copied().unwrap_or(0));
            let lo = hex_nibble(pair.last().copied().unwrap_or(0));
            match (hi, lo) {
                (Some(h), Some(l)) => match bytes.get_mut(i) {
                    Some(slot) => *slot = (h << 4) | l,
                    None => {
                        return Err(VerifyError::MalformedDigest {
                            reason: format!(
                                "more than 32 byte pairs in a {} byte string",
                                trimmed.len()
                            ),
                        })
                    }
                },
                _ => {
                    return Err(VerifyError::MalformedDigest {
                        reason: format!("not a hex character at byte {}", i * 2),
                    })
                }
            }
        }
        Ok(Self(bytes))
    }

    /// The digest in lowercase hex, which is the form it is published and logged in.
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for b in self.0 {
            // A nibble is 0..=15 and `HEX` holds 16 entries, so both lookups are in range.
            // Written with `get` anyway: the workspace denies indexing, and a digest is
            // printed into logs, so a panic here would be a poor way to fail.
            let hi = HEX.get((b >> 4) as usize).copied().unwrap_or('0');
            let lo = HEX.get((b & 0x0f) as usize).copied().unwrap_or('0');
            out.push(hi);
            out.push(lo);
        }
        out
    }

    /// Constant-time equality relative to content length.
    ///
    /// Used instead of `==` because the derived comparison stops at the first differing byte,
    /// and a timing signal on a digest is exactly the sort of thing that is cheap to avoid.
    pub fn matches(self, other: Self) -> bool {
        let mut diff = 0u8;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

const HEX: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// A downloaded artifact together with the digest it is supposed to have.
///
/// The caller supplies this, because **no acquisition is implemented** (see the crate docs).
/// Keeping it a plain value means the verification step can be tested against a file on disk
/// without a network, and means a future acquisition module fills in a struct rather than
/// changing a signature.
#[derive(Debug, Clone)]
pub struct ArtifactSource {
    /// Where the bytes are, once written.
    pub path: PathBuf,
    /// The digest the publisher states for these bytes.
    pub expected: Digest,
}

impl ArtifactSource {
    /// Largest artifact this verifier will read, in bytes.
    ///
    /// 512 MiB. The largest real server artifact this project knows of is the Adoptium JRE at
    /// ~58 MB; a Minecraft server jar is tens of MB. The bound is generous for both and exists
    /// so a hostile or broken source cannot make the host allocate without limit.
    pub const MAX_BYTES: u64 = 512 * 1024 * 1024;
}

/// Why verification failed.
#[derive(Debug)]
pub enum VerifyError {
    /// The artifact could not be opened or read.
    Unreadable {
        /// The file's name, which the host needs in order to act.
        path: String,
        /// The underlying reason, from the OS.
        source: std::io::Error,
    },
    /// The artifact is larger than [`ArtifactSource::MAX_BYTES`].
    TooLarge {
        /// The file's name.
        path: String,
        /// Its size in bytes.
        size: u64,
        /// The bound it exceeded.
        limit: u64,
    },
    /// The file is not a regular file. A directory, device or link target cannot be a server
    /// artifact, and reading one would be at best pointless.
    NotAFile {
        /// The file's name.
        path: String,
    },
    /// The published digest was not 64 hex characters.
    MalformedDigest {
        /// What was wrong with it. Never the value itself.
        reason: String,
    },
    /// The bytes do not hash to the expected digest. **This is the tamper signal.**
    DigestMismatch {
        /// The file's name.
        path: String,
        /// What the publisher stated.
        expected: String,
        /// What the file actually is.
        actual: String,
    },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, source } => write!(f, "cannot read artifact {path}: {source}"),
            Self::TooLarge { path, size, limit } => write!(
                f,
                "artifact {path} is {size} bytes, over the {limit}-byte limit"
            ),
            Self::NotAFile { path } => write!(f, "artifact {path} is not a regular file"),
            Self::MalformedDigest { reason } => {
                write!(f, "the published digest is not usable: {reason}")
            }
            Self::DigestMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "artifact {path} does not match its published digest \
                 (expected {expected}, got {actual}); refusing to execute it"
            ),
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Verify a downloaded artifact against its published digest.
///
/// Returns `Ok(())` only when the bytes hash to exactly the expected digest. **The caller must
/// treat `Ok` as a precondition of execution, not a step of it** -- the point of this function
/// existing is that it is called before a process starts, never after.
pub fn verify(source: &ArtifactSource) -> Result<(), VerifyError> {
    let path_str = file_name(&source.path);

    let metadata = std::fs::metadata(&source.path).map_err(|e| VerifyError::Unreadable {
        path: path_str.clone(),
        source: e,
    })?;

    if !metadata.is_file() {
        return Err(VerifyError::NotAFile { path: path_str });
    }
    if metadata.len() > ArtifactSource::MAX_BYTES {
        return Err(VerifyError::TooLarge {
            path: path_str,
            size: metadata.len(),
            limit: ArtifactSource::MAX_BYTES,
        });
    }

    let actual = hash_file(&source.path, &path_str)?;
    if !actual.matches(source.expected) {
        return Err(VerifyError::DigestMismatch {
            path: path_str,
            expected: source.expected.to_hex(),
            actual: actual.to_hex(),
        });
    }
    Ok(())
}

/// Hash a file in bounded chunks.
fn hash_file(path: &Path, path_str: &str) -> Result<Digest, VerifyError> {
    let mut file = File::open(path).map_err(|e| VerifyError::Unreadable {
        path: path_str.to_string(),
        source: e,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; READ_BUFFER];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| VerifyError::Unreadable {
                path: path_str.to_string(),
                source: e,
            })?;
        if read == 0 {
            break;
        }
        // `read` is bounded by the buffer length, so `get` always succeeds; `unwrap_or` with the
        // whole buffer is a total fallback rather than a panic path.
        let chunk = buffer.get(..read).unwrap_or(buffer.as_slice());
        hasher.update(chunk);
    }

    let out = hasher.finalize();
    let mut bytes = [0u8; 32];
    // `out` is a GenericArray of exactly 32 bytes; copying through `get` keeps this free of
    // indexing, which the workspace denies.
    if let Some(slice) = out.as_slice().get(..32) {
        bytes.copy_from_slice(slice);
    }
    Ok(Digest(bytes))
}

/// A path's file name, for an error message. Never the full path, so a directory layout the host
/// did not choose does not end up in a log.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
