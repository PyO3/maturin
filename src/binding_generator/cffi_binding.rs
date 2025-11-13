use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::str;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use fs_err as fs;
use fs_err::File;
use tempfile::TempDir;
use tempfile::tempdir;
use tracing::debug;

use crate::ModuleWriter;
use crate::PyProjectToml;
use crate::Target;
use crate::module_writer::ModuleWriterExt;
use crate::module_writer::write_python_part;
use crate::project_layout::ProjectLayout;
use crate::target::Os;

/// Creates the cffi module with the shared library, the cffi declarations and the cffi loader
#[allow(clippy::too_many_arguments)]
pub fn write_cffi_module(
    writer: &mut impl ModuleWriter,
    target: &Target,
    project_layout: &ProjectLayout,
    crate_dir: &Path,
    target_dir: &Path,
    module_name: &str,
    artifact: &Path,
    python: &Path,
    editable: bool,
    pyproject_toml: Option<&PyProjectToml>,
) -> Result<()> {
    let cffi_declarations = generate_cffi_declarations(crate_dir, target_dir, python)?;

    if !editable {
        write_python_part(writer, project_layout, pyproject_toml)
            .context("Failed to add the python module to the package")?;
    }

    let cffi_module_file_name = {
        let extension_name = &project_layout.extension_name;
        // https://cffi.readthedocs.io/en/stable/embedding.html#issues-about-using-the-so
        match target.target_os() {
            Os::Macos => format!("lib{extension_name}.dylib"),
            Os::Windows => format!("{extension_name}.dll"),
            _ => format!("lib{extension_name}.so"),
        }
    };
    let module;
    if let Some(python_module) = &project_layout.python_module {
        if editable {
            let base_path = python_module.join(&project_layout.extension_name);
            fs::create_dir_all(&base_path)?;
            let target = base_path.join(&cffi_module_file_name);
            fs::copy(artifact, &target).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                target.display()
            ))?;
            File::create(base_path.join("__init__.py"))?
                .write_all(cffi_init_file(&cffi_module_file_name).as_bytes())?;
            File::create(base_path.join("ffi.py"))?.write_all(cffi_declarations.as_bytes())?;
        }

        let relative = project_layout
            .rust_module
            .strip_prefix(python_module.parent().unwrap())
            .unwrap();
        module = relative.join(&project_layout.extension_name);
    } else {
        module = PathBuf::from(module_name);
        let type_stub = project_layout
            .rust_module
            .join(format!("{module_name}.pyi"));
        if type_stub.exists() {
            eprintln!("📖 Found type stub file at {module_name}.pyi");
            writer.add_file(module.join("__init__.pyi"), type_stub, false)?;
            writer.add_empty_file(module.join("py.typed"))?;
        }
    };

    if !editable || project_layout.python_module.is_none() {
        writer.add_bytes(
            module.join("__init__.py"),
            None,
            cffi_init_file(&cffi_module_file_name).as_bytes(),
            false,
        )?;
        writer.add_bytes(
            module.join("ffi.py"),
            None,
            cffi_declarations.as_bytes(),
            false,
        )?;
        writer.add_file(module.join(&cffi_module_file_name), artifact, true)?;
    }

    Ok(())
}

/// Glue code that exposes `lib`.
fn cffi_init_file(cffi_module_file_name: &str) -> String {
    format!(
        r#"__all__ = ["lib", "ffi"]

import os
from .ffi import ffi

lib = ffi.dlopen(os.path.join(os.path.dirname(__file__), '{cffi_module_file_name}'))
del os
"#
    )
}

/// Wraps some boilerplate around error handling when calling python
fn call_python<I, S>(python: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(python)
        .args(args)
        .output()
        .context(format!("Failed to run python at {:?}", &python))
}

/// Checks if user has provided their own header at `target/header.h`, otherwise
/// we run cbindgen to generate one.
fn cffi_header(crate_dir: &Path, target_dir: &Path, tempdir: &TempDir) -> Result<PathBuf> {
    let maybe_header = target_dir.join("header.h");

    if maybe_header.is_file() {
        eprintln!("💼 Using the existing header at {}", maybe_header.display());
        Ok(maybe_header)
    } else {
        if crate_dir.join("cbindgen.toml").is_file() {
            eprintln!(
                "💼 Using the existing cbindgen.toml configuration.\n\
                 💼 Enforcing the following settings:\n   \
                 - language = \"C\" \n   \
                 - no_includes = true, sys_includes = []\n     \
                   (#include is not yet supported by CFFI)\n   \
                 - defines = [], include_guard = None, pragma_once = false, cpp_compat = false\n     \
                   (#define, #ifdef, etc. is not yet supported by CFFI)\n"
            );
        }

        let mut config = cbindgen::Config::from_root_or_default(crate_dir);
        config.language = cbindgen::Language::C;
        config.no_includes = true;
        config.sys_includes = Vec::new();
        config.defines = HashMap::new();
        config.include_guard = None;
        config.pragma_once = false;
        config.cpp_compat = false;

        let bindings = cbindgen::Builder::new()
            .with_config(config)
            .with_crate(crate_dir)
            .with_language(cbindgen::Language::C)
            .with_no_includes()
            .generate()
            .context("Failed to run cbindgen")?;

        let header = tempdir.as_ref().join("header.h");
        bindings.write_to_file(&header);
        debug!("Generated header.h at {}", header.display());
        Ok(header)
    }
}

/// Returns the content of what will become ffi.py by invoking cbindgen and cffi
///
/// Checks if user has provided their own header at `target/header.h`, otherwise
/// we run cbindgen to generate one. Installs cffi if it's missing and we're inside a virtualenv
///
/// We're using the cffi recompiler, which reads the header, translates them into instructions
/// how to load the shared library without the header and then writes those instructions to a
/// file called `ffi.py`. This `ffi.py` will expose an object called `ffi`. This object is used
/// in `__init__.py` to load the shared library into a module called `lib`.
fn generate_cffi_declarations(
    crate_dir: &Path,
    target_dir: &Path,
    python: &Path,
) -> Result<String> {
    let tempdir = tempdir()?;
    let header = cffi_header(crate_dir, target_dir, &tempdir)?;

    let ffi_py = tempdir.as_ref().join("ffi.py");

    // Using raw strings is important because on windows there are path like
    // `C:\Users\JohnDoe\AppData\Local\TEmpl\pip-wheel-asdf1234` where the \U
    // would otherwise be a broken unicode escape sequence
    let cffi_invocation = format!(
        r#"
import cffi
from cffi import recompiler

ffi = cffi.FFI()
with open(r"{header}") as header:
    ffi.cdef(header.read())
recompiler.make_py_source(ffi, "ffi", r"{ffi_py}")
"#,
        ffi_py = ffi_py.display(),
        header = header.display(),
    );

    let output = call_python(python, ["-c", &cffi_invocation])?;
    let install_cffi = if !output.status.success() {
        // First, check whether the error was cffi not being installed
        let last_line = str::from_utf8(&output.stderr)?.lines().last().unwrap_or("");
        if last_line == "ModuleNotFoundError: No module named 'cffi'" {
            // Then check whether we're running in a virtualenv.
            // We don't want to modify any global environment
            // https://stackoverflow.com/a/42580137/3549270
            let output = call_python(
                python,
                ["-c", "import sys\nprint(sys.base_prefix != sys.prefix)"],
            )?;

            match str::from_utf8(&output.stdout)?.trim() {
                "True" => true,
                "False" => false,
                _ => {
                    eprintln!(
                        "⚠️ Failed to determine whether python at {:?} is running inside a virtualenv",
                        &python
                    );
                    false
                }
            }
        } else {
            false
        }
    } else {
        false
    };

    // If there was success or an error that was not missing cffi, return here
    if !install_cffi {
        return handle_cffi_call_result(python, tempdir, &ffi_py, &output);
    }

    eprintln!("⚠️ cffi not found. Trying to install it");
    // Call pip through python to don't do the wrong thing when python and pip
    // are coming from different environments
    let output = call_python(
        python,
        [
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            "cffi",
        ],
    )?;
    if !output.status.success() {
        bail!(
            "Installing cffi with `{:?} -m pip install cffi` failed: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\nPlease install cffi yourself.",
            &python,
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?
        );
    }
    eprintln!("🎁 Installed cffi");

    // Try again
    let output = call_python(python, ["-c", &cffi_invocation])?;
    handle_cffi_call_result(python, tempdir, &ffi_py, &output)
}

/// Extracted into a function because this is needed twice
fn handle_cffi_call_result(
    python: &Path,
    tempdir: TempDir,
    ffi_py: &Path,
    output: &Output,
) -> Result<String> {
    if !output.status.success() {
        bail!(
            "Failed to generate cffi declarations using {}: {}\n--- Stdout:\n{}\n--- Stderr:\n{}",
            python.display(),
            output.status,
            str::from_utf8(&output.stdout)?,
            str::from_utf8(&output.stderr)?,
        );
    } else {
        // Don't swallow warnings
        io::stderr().write_all(&output.stderr)?;

        let ffi_py_content = fs::read_to_string(ffi_py)?;
        tempdir.close()?;
        Ok(ffi_py_content)
    }
}
