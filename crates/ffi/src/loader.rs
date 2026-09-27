//! Dynamic-library loading and the ABI handshake (features F-A-01/02/
//! 04/08).
//!
//! The loader is the only place that talks to `libloading`. For every
//! requested library it runs the version handshake first
//! (`CCaller_abi_version` must exist and return
//! [`CCALLER_ABI_VERSION`], FR-A-04), then resolves each declared
//! `Call_<name>` symbol (FR-A-02). The open handles are owned by
//! [`LoadedFunctions`]; the resolved pointers stay valid only for as
//! long as that value lives.
//!
//! This module cannot depend on the core configuration model (AR-01
//! dependency direction `cli -> core -> ffi`), so callers translate
//! their library descriptions into [`LibraryRequest`]s with paths
//! already resolved against the description file's directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use libloading::{Library, Symbol};

use crate::abi::CCALLER_ABI_VERSION;

/// The unified call signature `Call_<name>` (requirement spec 7.1).
// SAFETY: the alias only ascribes the C calling convention fixed by
// requirement spec 7.1; calling a value of this type is the unsafe
// operation, annotated at every call site below.
pub type CallFn =
    unsafe extern "C" fn(param_page: *mut u64, params: *const i64, param_len: i64) -> i64;

/// The handshake signature `CCaller_abi_version` (FR-A-04).
// SAFETY: same as `CallFn` - the alias ascribes the contract
// signature; the one call site carries its own SAFETY note.
pub type AbiVersionFn = unsafe extern "C" fn() -> i64;

/// Symbol every wrapper must export for the handshake (FR-A-04).
const ABI_VERSION_SYMBOL: &[u8] = b"CCaller_abi_version";

/// Environment variable the OS consults when resolving a dynamic
/// library's own dependencies; named in load-failure hints (F-A-08).
const DEPENDENCY_SEARCH_ENV: &str = if cfg!(windows) {
    "PATH"
} else {
    "LD_LIBRARY_PATH"
};

/// Errors raised while loading wrapper libraries.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LoadError {
    /// The dynamic library, or one of its dependencies, could not be
    /// opened; the hint points at dependency resolution (F-A-08).
    #[error(
        "failed to load library `{path}`: {source}; hint: if the file exists, one of its \
         dependencies is missing - the OS resolves dependencies via {env_var}"
    )]
    LibraryOpen {
        /// Path of the library as given in the load request.
        path: PathBuf,
        /// The underlying loader failure.
        source: libloading::Error,
        /// OS environment variable that resolves dependencies.
        env_var: &'static str,
    },
    /// The library does not export `CCaller_abi_version`, so the
    /// handshake cannot run (FR-A-04).
    #[error(
        "library `{path}` does not export the required symbol \
         `CCaller_abi_version` (ABI handshake, FR-A-04)"
    )]
    MissingVersion {
        /// Path of the library that lacks the handshake symbol.
        path: PathBuf,
    },
    /// The library reports an ABI version different from the
    /// framework's; both numbers are reported (FR-A-04).
    #[error(
        "library `{path}` reports ABI version {found}, but this framework \
         expects {expected} (FR-A-04)"
    )]
    VersionMismatch {
        /// Path of the rejected library.
        path: PathBuf,
        /// Version the wrapper reported.
        found: i64,
        /// Version this framework requires.
        expected: i64,
    },
    /// A declared `Call_<name>` symbol does not exist in the library
    /// (FR-A-02).
    #[error("library `{path}` does not export function `{func}` (FR-A-02)")]
    MissingFunc {
        /// Path of the library that lacks the symbol.
        path: PathBuf,
        /// Name of the missing function.
        func: String,
    },
    /// The same function name is declared by two libraries (FR-A-01);
    /// the static configuration check reports this earlier, the loader
    /// re-checks as defense in depth.
    #[error(
        "function `{func}` is declared by both `{first}` and `{second}` \
         (FR-A-01)"
    )]
    DuplicateFunc {
        /// Name declared by both libraries.
        func: String,
        /// Path of the first declaring library.
        first: PathBuf,
        /// Path of the second declaring library.
        second: PathBuf,
    },
}

/// One library to load: an already-resolved path plus the exported
/// function names to resolve from it.
#[derive(Debug, Clone)]
pub struct LibraryRequest {
    /// Filesystem path of the dynamic library; relative paths must be
    /// resolved by the caller before building a request.
    pub path: PathBuf,
    /// Declared `Call_<name>` symbols to resolve from this library.
    pub func_names: Vec<String>,
}

/// A resolved `Call_<name>` entry: the function pointer plus the
/// library it came from, kept for diagnostics.
#[derive(Debug, Clone)]
pub struct ResolvedFunc {
    /// The unified-signature pointer; valid only while the owning
    /// [`LoadedFunctions`] value is alive.
    pub call: CallFn,
    /// Path of the library the symbol was resolved from.
    pub lib_path: PathBuf,
}

/// All loaded libraries and their resolved functions.
///
/// The struct owns the open library handles, so dropping it unloads
/// every wrapper; resolved pointers handed out earlier must not be
/// called afterwards. It is `Send + Sync`, letting worker threads
/// share one loaded set.
pub struct LoadedFunctions {
    /// Open handles backing the pointers in `functions`.
    libraries: Vec<Library>,
    /// Resolved `Call_<name>` pointers by symbol name.
    functions: HashMap<String, ResolvedFunc>,
}

impl std::fmt::Debug for LoadedFunctions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Library` has no Debug impl; report the handle count and the
        // resolved symbol names instead of raw pointers.
        let mut names: Vec<&str> = self.functions.keys().map(String::as_str).collect();
        names.sort_unstable();
        f.debug_struct("LoadedFunctions")
            .field("libraries", &self.libraries.len())
            .field("functions", &names)
            .finish()
    }
}

impl LoadedFunctions {
    /// Load every requested library, run the handshake on each, and
    /// resolve all declared functions.
    ///
    /// Libraries are processed in request order; the first failure
    /// aborts the whole load (a partially loaded set is never
    /// returned).
    pub fn load(requests: &[LibraryRequest]) -> Result<Self, LoadError> {
        let mut libraries = Vec::with_capacity(requests.len());
        let mut functions: HashMap<String, ResolvedFunc> = HashMap::new();
        for request in requests {
            // SAFETY: opening a dynamic library may execute its
            // initializers (DllMain / ELF constructors). The path comes
            // from the user's own library description, so the user has
            // explicitly asked to run this code; that is the tool's
            // purpose and the accepted trust boundary.
            let library = unsafe { Library::new(&request.path) }.map_err(|source| {
                LoadError::LibraryOpen {
                    path: request.path.clone(),
                    source,
                    env_var: DEPENDENCY_SEARCH_ENV,
                }
            })?;
            handshake(&library, &request.path)?;
            for name in &request.func_names {
                if let Some(existing) = functions.get(name) {
                    return Err(LoadError::DuplicateFunc {
                        func: name.clone(),
                        first: existing.lib_path.clone(),
                        second: request.path.clone(),
                    });
                }
                let call = resolve_call(&library, name, &request.path)?;
                functions.insert(
                    name.clone(),
                    ResolvedFunc {
                        call,
                        lib_path: request.path.clone(),
                    },
                );
            }
            libraries.push(library);
        }
        Ok(Self {
            libraries,
            functions,
        })
    }

    /// Look up a resolved `Call_<name>` by symbol name.
    pub fn get(&self, name: &str) -> Option<&ResolvedFunc> {
        self.functions.get(name)
    }

    /// Number of wrapper libraries currently held open.
    ///
    /// The handles live as long as this value; the count is a
    /// diagnostics surface for logging and reports.
    pub fn library_count(&self) -> usize {
        self.libraries.len()
    }
}

/// Resolve and call `CCaller_abi_version`, verifying the handshake
/// (FR-A-04). Errors name the offending library.
fn handshake(library: &Library, path: &Path) -> Result<(), LoadError> {
    // SAFETY: the lookup uses the exact handshake symbol name and
    // ascribes the signature fixed by requirement spec 7.1, which
    // every wrapper contractually exports with C linkage; the
    // `Symbol` borrow keeps the borrowed `library` loaded.
    let symbol: Symbol<'_, AbiVersionFn> = unsafe {
        library
            .get(ABI_VERSION_SYMBOL)
            .map_err(|_| LoadError::MissingVersion {
                path: path.to_owned(),
            })?
    };
    let version_fn: AbiVersionFn = *symbol;
    // SAFETY: the callee takes no arguments and only returns an
    // integer, so memory safety holds as long as the signature matches
    // the contract (guaranteed by the wrapper including `ccaller.h`,
    // which declares the symbol) and the library stays loaded; the
    // `library` borrow is still live here.
    let found = unsafe { version_fn() };
    if found != CCALLER_ABI_VERSION {
        return Err(LoadError::VersionMismatch {
            path: path.to_owned(),
            found,
            expected: CCALLER_ABI_VERSION,
        });
    }
    Ok(())
}

/// Resolve one `Call_<name>` symbol (FR-A-02). Errors name the
/// function and the library path.
fn resolve_call(library: &Library, name: &str, path: &Path) -> Result<CallFn, LoadError> {
    // SAFETY: the lookup uses the declared symbol name, and the type
    // ascribes the unified signature fixed by requirement spec 7.1,
    // which every declared `Call_<name>` contractually has; as with
    // the handshake, the `Symbol` borrow ties the pointer's validity
    // to the still-borrowed `library`.
    let symbol: Symbol<'_, CallFn> = unsafe {
        library
            .get(name.as_bytes())
            .map_err(|_| LoadError::MissingFunc {
                path: path.to_owned(),
                func: name.to_owned(),
            })?
    };
    Ok(*symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_search_env_matches_the_platform__F_A_08() {
        let expected = if cfg!(windows) {
            "PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        assert_eq!(DEPENDENCY_SEARCH_ENV, expected);
    }

    #[test]
    fn version_mismatch_message_reports_both_numbers__F_A_04() {
        let error = LoadError::VersionMismatch {
            path: PathBuf::from("libs/wrapper.so"),
            found: 2,
            expected: 1,
        };
        let text = error.to_string();
        assert!(text.contains("2"), "reports the wrapper version: {text}");
        assert!(text.contains("1"), "reports the framework version: {text}");
    }

    #[test]
    fn library_open_message_names_the_env_hint__F_A_08() {
        let error = LoadError::LibraryOpen {
            path: PathBuf::from("libs/wrapper.so"),
            source: libloading::Error::IncompatibleSize,
            env_var: DEPENDENCY_SEARCH_ENV,
        };
        let text = error.to_string();
        assert!(
            text.contains(DEPENDENCY_SEARCH_ENV),
            "names the env: {text}"
        );
    }
}
