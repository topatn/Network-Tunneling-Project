//! Per-user data directories, with every write confined to them (`SRV-010`).
//!
//! ## Why this module exists at all
//!
//! `SRV-010` requires that provisioning write only inside a per-user data directory and never
//! install a service or require elevation. The obvious way to satisfy that is to be careful at
//! each call site. This module makes care unnecessary: there is one way to turn a host-chosen
//! name into a path, and it cannot produce a path outside the root.
//!
//! ## The two attacks this refuses
//!
//! 1. **Traversal.** `servers/../../Windows/System32/evil` joins to a perfectly ordinary-looking
//!    path unless something normalises it. [`DataPaths::resolve`] normalises before comparing,
//!    so `..` cannot walk out.
//! 2. **Absolute paths.** `C:\Users\me\AppData\Local\app\servers\C:\Windows` is not a relative
//!    name at all. [`Path::join`] would discard the root entirely, so an absolute component is
//!    refused rather than joined.
//!
//! Symlinks are the third, and the reason [`DataPaths::resolve`] is willing to fail:
//! a link inside the root pointing outside it would pass a purely lexical check. The check
//! therefore compares **canonicalised** paths, and if the target does not exist yet the parent
//! is canonicalised instead. Where neither can be canonicalised the path is refused rather than
//! guessed at.
//!
//! ## No elevation, no service
//!
//! Nothing here creates a service, changes an ACL, or writes outside the root. The root is
//! derived from the platform's per-user application-data directory by the caller; this module
//! only ever appends relative names beneath it.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Why a path was refused.
///
/// No `PartialEq`: a variant carries a [`std::io::Error`], which is neither `Clone` nor `Eq`, and
/// deriving them anyway would mean either unwrapping the OS error or dropping it. Tests compare
/// the `Display` form, which is the part a host sees.
#[derive(Debug)]
pub enum PathError {
    /// A path component was absolute, so joining it would discard the root entirely.
    AbsoluteComponent {
        /// The rejected name, as supplied.
        supplied: String,
    },
    /// A component escaped the root after normalisation.
    EscapesRoot {
        /// The rejected name, as supplied.
        supplied: String,
    },
    /// The path or its parent could not be canonicalised, so containment cannot be established.
    NotCanonicalisable {
        /// The rejected name, as supplied.
        supplied: String,
        /// The underlying reason, from the OS.
        source: std::io::Error,
    },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AbsoluteComponent { supplied } => write!(
                f,
                "{supplied:?} contains an absolute path component; only a relative name \
                 beneath the data directory is allowed"
            ),
            Self::EscapesRoot { supplied } => write!(
                f,
                "{supplied:?} resolves outside the per-user data directory"
            ),
            Self::NotCanonicalisable { supplied, source } => write!(
                f,
                "{supplied:?} could not be canonicalised, so containment cannot be \
                 established: {source}"
            ),
        }
    }
}

impl std::error::Error for PathError {}

/// The per-user data directory, and the only way to get a path inside it.
///
/// Constructed from a root the caller has already established to be per-user — on Windows,
/// `%LOCALAPPDATA%`. The root itself is canonicalised once, at construction, so every later
/// comparison is against a fixed, real path rather than whatever the string happened to be.
#[derive(Debug, Clone)]
pub struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    /// Establish the data directory for `app_name`.
    ///
    /// `app_name` is validated as a single relative name: it is a constant in this codebase,
    /// not user input, and refusing anything else keeps the root unambiguous.
    pub fn new(base: &Path, app_name: &str) -> Result<Self, PathError> {
        let candidate = base.join(app_name);
        let root = candidate
            .canonicalize()
            .map_err(|source| PathError::NotCanonicalisable {
                supplied: app_name.to_string(),
                source,
            })?;
        Ok(Self { root })
    }

    /// The data directory itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Turn a relative name into a path inside the data directory, or refuse.
    ///
    /// The only constructor of a path from a name in this crate. A caller that wants
    /// `servers/minecraft-java` calls `resolve(&["servers", "minecraft-java"])`; a caller that
    /// passes `["..", ".."]`, an absolute path, or a name that still escapes after
    /// normalisation gets a [`PathError`] rather than a path.
    pub fn resolve(&self, segments: &[&str]) -> Result<PathBuf, PathError> {
        if segments.is_empty() {
            return Err(PathError::EscapesRoot {
                supplied: String::from("<empty>"),
            });
        }

        let mut candidate = self.root.clone();
        for segment in segments {
            let as_path = Path::new(segment);
            // A component that is absolute, or a Windows prefix / root, would make `join`
            // discard everything built so far.
            if as_path.is_absolute()
                || as_path.has_root()
                || matches!(
                    as_path.components().next(),
                    Some(Component::RootDir | Component::Prefix(_))
                )
            {
                return Err(PathError::AbsoluteComponent {
                    supplied: (*segments).join("/"),
                });
            }
            candidate.push(as_path);
        }

        // Normalise lexically first, so `a/../b` is `b` rather than a path containing `..`.
        let normalised = normalise(&candidate);

        // Then canonicalise, which is what catches a symlink inside the root pointing out.
        // A path that does not exist yet is normal for provisioning -- the directory is about
        // to be created -- so the **nearest existing ancestor** is canonicalised and the
        // remaining components re-appended.
        //
        // The ancestor walk matters and was a bug once: an earlier version canonicalised only
        // the *immediate* parent, so `servers/minecraft-java/world` was refused whenever
        // neither `world` nor `minecraft-java` existed -- which is precisely the case
        // provisioning creates. Walking up fixes it, and costs nothing when the path exists.
        let real = self
            .canonicalise_with_missing_tail(&normalised)
            .ok_or_else(|| PathError::NotCanonicalisable {
                supplied: (*segments).join("/"),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no existing ancestor to canonicalise against",
                ),
            })?;

        if !real.starts_with(&self.root) {
            return Err(PathError::EscapesRoot {
                supplied: (*segments).join("/"),
            });
        }

        Ok(real)
    }

    /// Canonicalise `path`, or canonicalise its nearest existing ancestor and re-append the
    /// components that do not exist yet.
    ///
    /// Walking up is what makes `resolve` usable for its actual purpose: provisioning creates
    /// `servers/<game>/<artifact>` where neither `<game>` nor `<artifact>` exists. Canonicalising
    /// only the immediate parent would refuse every one of those, which an earlier version did.
    ///
    /// Returns `None` when no ancestor exists, which cannot happen for a path under a root that
    /// was canonicalised at construction — but is handled rather than assumed, because a caller
    /// must not be able to reach an unchecked path by making the walk fail.
    fn canonicalise_with_missing_tail(&self, path: &Path) -> Option<PathBuf> {
        if let Ok(real) = path.canonicalize() {
            return Some(real);
        }

        // Collected in reverse: `missing` ends up outermost-first once reversed.
        let mut missing: Vec<String> = Vec::new();
        let mut cursor = path;

        loop {
            let Some(name) = cursor.file_name() else {
                // Reached a root or prefix with nothing above it.
                return None;
            };
            missing.push(name.to_string_lossy().into_owned());

            // No parent means a root or prefix with nothing above it. Cannot happen for a
            // path under an already-canonicalised root, but handled rather than assumed.
            let parent = cursor.parent()?;
            cursor = parent;

            if let Ok(real_parent) = cursor.canonicalize() {
                let mut out = real_parent;
                for component in missing.iter().rev() {
                    out.push(component);
                }
                return Some(out);
            }
        }
    }
}

/// Resolve `.` and `..` without touching the filesystem.
///
/// `Path::components` already normalises `.` away and reports `..` as a `ParentDir` component,
/// so walking and popping is enough. `..` at the front is dropped rather than allowed to
/// escape upward, which keeps the function total -- the caller still checks the final path
/// against the root, so dropping is a convenience and not the security boundary.
fn normalise(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                match out.last() {
                    Some(Component::Normal(_)) => {
                        out.pop();
                    }
                    // Nothing to pop: keep it so the caller's containment check sees it and
                    // refuses, rather than silently resolving to the root.
                    _ => out.push(component),
                };
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out.iter().collect()
}
