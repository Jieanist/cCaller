//! The `init` subcommand's scaffolding (developer tooling).
//!
//! The templates form a fuller starting project than the README's minimal
//! hello-world: they demonstrate `slot_roles` (read/write) and shared-input
//! reuse (`refs`). `build.sh` is optional (`--build-sh`).

use std::fmt;
use std::fs;
use std::path::Path;

/// The library description template (demonstrates `slot_roles` read/write).
///
/// The library file name follows the platform: `wrapper.dll` on Windows,
/// `wrapper.so` elsewhere (requirement spec 7.2).
pub fn libs_template() -> String {
    let lib_file = if cfg!(windows) {
        "wrapper.dll"
    } else {
        "wrapper.so"
    };
    format!(
        "# libs.toml —— 库描述\n\
         version = 1\n\
         \n\
         [[libs]]\n\
         path = \"{lib_file}\"   # 相对本文件\n\
         funcs = [\n\
         \x20 {{ name = \"Call_malloc\", paras = [\"len\", \"mem_idx\"], slot_roles = {{ mem_idx = \"write\" }} }},\n\
         \x20 {{ name = \"Call_read32\", paras = [\"addr_idx\"],      slot_roles = {{ addr_idx = \"read\" }} }},\n\
         ]\n"
    )
}

/// The case configuration template (demonstrates shared-input reuse via `refs`).
pub fn cases_template() -> String {
    "# cases.toml —— 用例配置\n\
     version = 1\n\
     \n\
     [shared_inputs.common]\n\
     val = [\"888\", \"999\"]\n\
     \n\
     [[tests]]\n\
     name = \"test_rw_u32\"\n\
     cmds = [\n\
     \x20 { opfunc = \"Call_malloc\", expect_eq = 0,      args = [\"len=100\", \"mem_idx=1\"] },\n\
     \x20 { opfunc = \"Call_read32\", expect_eq = \"$val\", args = [\"addr_idx=1\"] },\n\
     ]\n\
     [[tests.inputs]]\n\
     name = \"ipt1\"\n\
     refs = [\"common\"]   # 复用 shared_inputs.common → 2 个子用例：val ∈ {888, 999}\n"
        .to_string()
}

/// The optional build script template.
pub fn build_sh_template() -> String {
    "#!/bin/sh\n\
     # 把本目录的 C 源文件编译成 wrapper 库（按平台命名）。\n\
     set -e\n\
     cc -shared -fPIC -o wrapper.so *.c\n"
        .to_string()
}

/// Why a scaffold failed.
#[derive(Debug)]
pub enum ScaffoldError {
    /// A target file already exists; nothing was written.
    Exists(String),
    /// A directory could not be created or a file not written.
    Io {
        path: String,
        source: std::io::Error,
    },
}

impl fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScaffoldError::Exists(path) => {
                write!(f, "`{path}` already exists; refusing to overwrite it")
            }
            ScaffoldError::Io { path, source } => {
                write!(f, "failed to write `{path}`: {source}")
            }
        }
    }
}

impl std::error::Error for ScaffoldError {}

/// Writes the scaffold into `dir`, creating the directory if needed.
///
/// Nothing is written unless every target is free: an existing file is
/// reported through [`ScaffoldError::Exists`] instead of being silently
/// overwritten. On success the written paths are returned in order.
///
/// # Errors
/// See [`ScaffoldError`].
pub fn scaffold(dir: &Path, with_build_sh: bool) -> Result<Vec<String>, ScaffoldError> {
    // Refuse before writing anything: check every target first.
    for name in target_names(with_build_sh) {
        let path = dir.join(name);
        if path.exists() {
            return Err(ScaffoldError::Exists(path.to_string_lossy().into_owned()));
        }
    }
    fs::create_dir_all(dir).map_err(|source| ScaffoldError::Io {
        path: dir.to_string_lossy().into_owned(),
        source,
    })?;

    let write = |name: &str, content: String| -> Result<String, ScaffoldError> {
        let path = dir.join(name);
        fs::write(&path, content).map_err(|source| ScaffoldError::Io {
            path: path.to_string_lossy().into_owned(),
            source,
        })?;
        Ok(path.to_string_lossy().into_owned())
    };

    let mut written = vec![
        write("libs.toml", libs_template())?,
        write("cases.toml", cases_template())?,
    ];
    if with_build_sh {
        let path = write("build.sh", build_sh_template())?;
        // The script is meant to be run, not sourced.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let full = dir.join("build.sh");
            let mut permissions =
                fs::metadata(&full)
                    .map(|meta| meta.permissions())
                    .map_err(|source| ScaffoldError::Io {
                        path: full.to_string_lossy().into_owned(),
                        source,
                    })?;
            permissions.set_mode(0o755);
            fs::set_permissions(&full, permissions).map_err(|source| ScaffoldError::Io {
                path: full.to_string_lossy().into_owned(),
                source,
            })?;
        }
        written.push(path);
    }
    Ok(written)
}

/// The file names a scaffold would create.
fn target_names(with_build_sh: bool) -> &'static [&'static str] {
    if with_build_sh {
        &["libs.toml", "cases.toml", "build.sh"]
    } else {
        &["libs.toml", "cases.toml"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_match_the_readme_example__F_X_04() {
        let libs = libs_template();
        assert!(libs.contains("version = 1"), "{libs}");
        assert!(libs.contains("Call_malloc"), "{libs}");
        assert!(libs.contains("slot_roles"), "{libs}");
        // Platform-appropriate library name (spec 7.2).
        let expected = if cfg!(windows) {
            "wrapper.dll"
        } else {
            "wrapper.so"
        };
        assert!(libs.contains(expected), "{libs}");

        let cases = cases_template();
        assert!(cases.contains("[shared_inputs.common]"), "{cases}");
        assert!(cases.contains("\"$val\""), "{cases}");
        assert!(cases.contains("[[tests.inputs]]"), "{cases}");
        assert!(cases.contains("refs = [\"common\"]"), "{cases}");
    }

    #[test]
    fn the_cases_template_is_a_valid_configuration__F_X_04() {
        // The scaffold must be immediately checkable: parse it against
        // the strict model.
        let doc = ccaller_core::config::SourceDoc {
            path: "cases.toml".to_string(),
            text: cases_template(),
        };
        doc.parse::<ccaller_core::config::CaseConfig>()
            .expect("the template must parse");
    }

    #[test]
    fn the_libs_template_is_a_valid_description__F_X_04() {
        let doc = ccaller_core::config::SourceDoc {
            path: "libs.toml".to_string(),
            text: libs_template(),
        };
        doc.parse::<ccaller_core::config::LibDescription>()
            .expect("the template must parse");
    }

    #[test]
    fn scaffold_refuses_to_overwrite_and_writes_nothing__F_X_04() {
        let dir = std::env::temp_dir().join(format!("ccaller-init-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cases.toml"), "keep me").unwrap();
        let result = scaffold(&dir, false);
        let Err(ScaffoldError::Exists(path)) = result else {
            panic!("the existing cases.toml must be refused");
        };
        assert!(path.ends_with("cases.toml"), "{path}");
        // The other target was not created either.
        assert!(!dir.join("libs.toml").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("cases.toml")).unwrap(),
            "keep me"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scaffold_writes_all_three_files_when_asked__F_X_04() {
        let dir = std::env::temp_dir().join(format!("ccaller-init-full-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let written = scaffold(&dir, true).expect("a fresh directory accepts the scaffold");
        assert_eq!(written.len(), 3);
        assert!(dir.join("libs.toml").exists());
        assert!(dir.join("cases.toml").exists());
        assert!(dir.join("build.sh").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scaffold_creates_a_missing_directory__F_X_04() {
        let dir = std::env::temp_dir()
            .join(format!("ccaller-init-mkdir-{}", std::process::id()))
            .join("nested");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        scaffold(&dir, false).expect("a missing directory is created");
        assert!(dir.join("cases.toml").exists());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
