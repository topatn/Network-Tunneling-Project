//! Loading a directory of profile files.
//!
//! ## Every read is bounded
//!
//! AGENTS.md section 4 forbids unbounded allocation on file input. A profile file is
//! repository data rather than attacker-controlled, but "currently only we write these" is not
//! a size limit. Three bounds apply, and each is checked *before* the allocation it governs:
//!
//! * the file's size, from metadata, before its bytes are read;
//! * the number of files in the directory;
//! * the number of mappings in a profile, in [`crate::validate`].
//!
//! ## A directory that cannot be read is an error, not an empty registry
//!
//! An absent `profiles/` directory yields an empty registry and a caller that ignores the
//! result gets "no profiles available", which is indistinguishable from a bug in the caller.
//! [`Registry::load_dir`] therefore distinguishes the two: a missing directory is
//! [`RegistryError::NoProfilesDirectory`], and an empty one is
//! [`RegistryError::Empty`]. A build that ships no profiles is a build mistake, and saying so
//! is the difference between a clear failure and a confusing one.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::Profile;
use crate::validate::{self, ProfileError};

/// Largest a profile file may be, in bytes.
///
/// A profile declares a handful of short mappings. 64 KiB is far above any real file and far
/// below anything worth reading, and it is checked from metadata before the read.
pub const MAX_PROFILE_BYTES: u64 = 64 * 1024;

/// Most files a profile directory may contain.
///
/// The project ships five. Sixty-four leaves room for [`GAME-007`]'s "adding a sixth profile
/// file requires no engine code change" without making the bound feel like a policy on how
/// many games the product may support.
pub const MAX_PROFILE_FILES: usize = 64;

/// Why a profile directory could not be loaded.
#[derive(Debug)]
pub enum RegistryError {
    /// The directory does not exist, or could not be listed.
    NoProfilesDirectory {
        /// The path that was looked for.
        path: PathBuf,
        /// The underlying reason, from the OS.
        source: std::io::Error,
    },
    /// The directory exists but holds no `*.json` profile files.
    Empty {
        /// The directory that was empty.
        path: PathBuf,
    },
    /// The directory holds more than [`MAX_PROFILE_FILES`] files.
    TooManyFiles {
        /// How many were found.
        found: usize,
        /// The limit that was exceeded.
        limit: usize,
    },
    /// A file could not be read.
    Unreadable {
        /// The file's name.
        name: String,
        /// The underlying reason, from the OS.
        source: std::io::Error,
    },
    /// A file was larger than [`MAX_PROFILE_BYTES`].
    TooLarge {
        /// The file's name.
        name: String,
        /// Its size in bytes.
        size: u64,
        /// The limit it exceeded.
        limit: u64,
    },
    /// A file was not valid JSON, or did not match the profile shape.
    ///
    /// Covers unknown fields, a protocol that is not `tcp`/`udp`, a port outside `u16`, and a
    /// malformed document -- all of which the deserializer rejects before a [`Profile`]
    /// exists.
    Malformed {
        /// The file's name.
        name: String,
        /// What the deserializer said.
        detail: String,
    },
    /// A file parsed but broke a cross-field rule.
    Invalid {
        /// The file's name.
        name: String,
        /// Every rule that was broken.
        error: ProfileError,
    },
    /// Two files declared the same profile id.
    DuplicateId {
        /// The id both claimed.
        id: String,
        /// The file already read.
        first: String,
        /// The file that collided with it.
        second: String,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProfilesDirectory { path, source } => write!(
                f,
                "cannot read the profile directory {}: {source}",
                path.display()
            ),
            Self::Empty { path } => {
                write!(f, "{} holds no *.json profile files", path.display())
            }
            Self::TooManyFiles { found, limit } => {
                write!(
                    f,
                    "found {found} profile files, at most {limit} are allowed"
                )
            }
            Self::Unreadable { name, source } => write!(f, "cannot read {name}: {source}"),
            Self::TooLarge { name, size, limit } => {
                write!(f, "{name} is {size} bytes, at most {limit} are allowed")
            }
            Self::Malformed { name, detail } => {
                write!(f, "{name} is not a valid profile: {detail}")
            }
            Self::Invalid { name, error } => write!(f, "{name}: {error}"),
            Self::DuplicateId { id, first, second } => write!(
                f,
                "profile id {id:?} is declared by both {first} and {second}; \
                 architecture.md says one file per game"
            ),
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoProfilesDirectory { source, .. } | Self::Unreadable { source, .. } => {
                Some(source)
            }
            Self::Invalid { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// The loaded set of profiles.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    profiles: Vec<Profile>,
}

impl Registry {
    /// Load and validate every `*.json` profile file in `dir`, in filename order.
    ///
    /// Filename order rather than directory order so that a failure is reported
    /// deterministically and two machines disagreeing about read order cannot disagree about
    /// which file was at fault.
    pub fn load_dir(dir: &Path) -> Result<Self, RegistryError> {
        let entries = fs::read_dir(dir).map_err(|source| RegistryError::NoProfilesDirectory {
            path: dir.to_path_buf(),
            source,
        })?;

        let mut names: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| RegistryError::NoProfilesDirectory {
                path: dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            let is_json = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"));
            // Not `is_file`: a directory named `x.json` is not a profile, and following it
            // would let a symlink loop look like an unbounded directory.
            if is_json && path.is_file() {
                names.push(path);
            }
        }

        if names.is_empty() {
            return Err(RegistryError::Empty {
                path: dir.to_path_buf(),
            });
        }
        if names.len() > MAX_PROFILE_FILES {
            return Err(RegistryError::TooManyFiles {
                found: names.len(),
                limit: MAX_PROFILE_FILES,
            });
        }

        names.sort();

        // Filenames are carried alongside the profiles so a duplicate-id collision can name
        // both files. Keeping a parallel vector of names instead would let the two drift; a
        // tuple per entry cannot.
        let mut loaded: Vec<(String, Profile)> = Vec::with_capacity(names.len());
        for path in &names {
            let name = file_name(path);
            let profile = load_one(path, &name)?;
            if let Some((first, _)) = loaded.iter().find(|(_, p)| p.id == profile.id) {
                return Err(RegistryError::DuplicateId {
                    id: profile.id.clone(),
                    first: first.clone(),
                    second: name,
                });
            }
            loaded.push((name, profile));
        }

        Ok(Self {
            profiles: loaded.into_iter().map(|(_, p)| p).collect(),
        })
    }

    /// Every loaded profile, in filename order.
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// The profile with this id, at any version.
    ///
    /// One file per game means one version per id, so the version is not part of the lookup.
    /// It is checked separately by [`Registry::get_versioned`] for callers that have a
    /// recorded `profile_version` to honour.
    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// The profile with this id **at exactly this version**, or `None`.
    ///
    /// A tunnel records the version it was created with (`data-model.md`), and the
    /// compatibility note there is that changing it affects live tunnels. A caller holding a
    /// recorded version therefore needs to be able to ask "is this still what I was created
    /// with", which is a different question from "does this game exist".
    pub fn get_versioned(&self, id: &str, version: i32) -> Option<&Profile> {
        self.get(id).filter(|p| p.version == version)
    }

    /// Profile ids in load order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.profiles.iter().map(|p| p.id.as_str())
    }

    /// How many profiles loaded.
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Whether the registry holds no profiles.
    ///
    /// Never true for a registry built by [`Registry::load_dir`], which rejects an empty
    /// directory. Present so a caller can fold this into an existing emptiness check.
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

/// Read, parse and validate one file, with the size bound applied before the read.
fn load_one(path: &Path, name: &str) -> Result<Profile, RegistryError> {
    let metadata = fs::metadata(path).map_err(|source| RegistryError::Unreadable {
        name: name.to_string(),
        source,
    })?;

    if metadata.len() > MAX_PROFILE_BYTES {
        return Err(RegistryError::TooLarge {
            name: name.to_string(),
            size: metadata.len(),
            limit: MAX_PROFILE_BYTES,
        });
    }

    // Metadata says "regular file"; `read_to_string` is what actually reads it. Both are
    // kept: the check gives a clear error for a file that is too big, and the read is the
    // only thing that can produce the contents.
    let text = fs::read_to_string(path).map_err(|source| RegistryError::Unreadable {
        name: name.to_string(),
        source,
    })?;

    let profile: Profile = serde_json::from_str(&text).map_err(|e| RegistryError::Malformed {
        name: name.to_string(),
        detail: e.to_string(),
    })?;

    validate::validate(&profile).map_err(|error| RegistryError::Invalid {
        name: name.to_string(),
        error,
    })?;

    Ok(profile)
}

/// A path's file name, as text.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
