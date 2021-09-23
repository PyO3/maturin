use crate::build_context::BridgeModel;
use crate::compile::compile;
use crate::module_writer::{write_bindings_module, write_cffi_module, PathWriter};
use crate::PythonInterpreter;
use crate::Target;
use crate::{write_dist_info, BuildOptions, Metadata21};
use crate::{ModuleWriter, PlatformTag};
use anyhow::{anyhow, bail, format_err, Context, Result};
use fs_err as fs;
#[cfg(not(target_os = "windows"))]
use std::fs::OpenOptions;
#[cfg(target_os = "windows")]
use std::io::Cursor;
use std::io::Write;
#[cfg(not(target_os = "windows"))]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;

// Windows launcher comes from https://bitbucket.org/vinay.sajip/simple_launcher/
// Pre-compiled binaries come from https://github.com/vsajip/distlib/blob/master/distlib/
#[cfg(target_os = "windows")]
static WIN_LAUNCHER_T32: &[u8] = include_bytes!("resources/t32.exe");
#[cfg(target_os = "windows")]
static WIN_LAUNCHER_T64: &[u8] = include_bytes!("resources/t64.exe");
#[cfg(target_os = "windows")]
static WIN_LAUNCHER_W32: &[u8] = include_bytes!("resources/w32.exe");
#[cfg(target_os = "windows")]
static WIN_LAUNCHER_W64: &[u8] = include_bytes!("resources/w64.exe");
#[cfg(target_os = "windows")]
static WIN_LAUNCHER_T64_ARM: &[u8] = include_bytes!("resources/t64-arm.exe");
#[cfg(target_os = "windows")]
static WIN_LAUNCHER_W64_ARM: &[u8] = include_bytes!("resources/w64-arm.exe");

/// Installs a crate by compiling it and copying the shared library to site-packages.
/// Also adds the dist-info directory to make sure pip and other tools detect the library
///
/// Works only in a virtualenv.
#[allow(clippy::too_many_arguments)]
pub fn develop(
    bindings: Option<String>,
    manifest_file: &Path,
    cargo_extra_args: Vec<String>,
    rustc_extra_args: Vec<String>,
    venv_dir: &Path,
    release: bool,
    strip: bool,
    extras: Vec<String>,
) -> Result<()> {
    let target = Target::from_target_triple(None)?;

    let python = target.get_venv_python(&venv_dir);

    let build_options = BuildOptions {
        platform_tag: Some(PlatformTag::Linux),
        interpreter: Some(vec![target.get_python()]),
        bindings,
        manifest_path: manifest_file.to_path_buf(),
        out: None,
        skip_auditwheel: false,
        target: None,
        cargo_extra_args,
        rustc_extra_args,
        universal2: false,
    };

    let build_context = build_options.into_build_context(release, strip)?;

    let interpreter = PythonInterpreter::check_executable(python, &target, &build_context.bridge)?
        .ok_or_else(|| {
            anyhow!("Expected `python` to be a python interpreter inside a virtualenv ಠ_ಠ")
        })?;

    // Install dependencies
    if !build_context.metadata21.requires_dist.is_empty() {
        let mut args = vec!["-m".to_string(), "pip".to_string(), "install".to_string()];
        args.extend(build_context.metadata21.requires_dist.iter().map(|x| {
            let mut pkg = x.clone();
            // Remove extra marker to make it installable with pip
            for extra in &extras {
                pkg = pkg
                    .replace(&format!(" and extra == '{}'", extra), "")
                    .replace(&format!("; extra == '{}'", extra), "");
            }
            pkg
        }));
        let status = Command::new(&interpreter.executable)
            .args(&args)
            .status()
            .context("Failed to run pip install")?;
        if !status.success() {
            bail!(r#"pip install finished with "{}""#, status)
        }
    }

    // First, uninstall the existing installation. Without this, the following can happen:
    // * `pip install my-project`: Creates files that maturin would not create, e.g. `my_project-1.0.0.dist-info/direct_url.json`
    // * `maturin develop`: Overwrites a RECORD file with one that doesn't list direct_url.json, while not removing the file
    // * `pip uninstall my-project`: Removes most things, but notes that direct_url.json won't be removed
    // * Any other pip action: Complains about my-project, crashes when trying to do anything with my-project
    //
    // Uninstalling the actual code is done individually for each bridge model
    let base_path = target.get_venv_site_package(venv_dir, &interpreter);
    let dist_info_dir = base_path.join(build_context.metadata21.get_dist_info_dir());
    if dist_info_dir.is_dir() {
        fs::remove_dir_all(&dist_info_dir).context(format!(
            "Failed to uninstall existing installation by removing {}",
            dist_info_dir.display()
        ))?;
    }

    let mut writer = PathWriter::venv(&target, venv_dir, &build_context.bridge)?;

    let context = "Failed to build a native library through cargo";

    match build_context.bridge {
        BridgeModel::Bin => {
            let artifacts = compile(&build_context, None, &BridgeModel::Bin).context(context)?;

            let artifact = artifacts
                .get("bin")
                .ok_or_else(|| format_err!("Cargo didn't build a binary"))?;

            // Copy the artifact into the same folder as pip and python
            let bin_name = artifact.file_name().unwrap();
            let bin_path = target.get_venv_bin_dir(&venv_dir).join(bin_name);
            // No need to uninstall since we're overwriting anyway
            // TODO: Shouldn't this use a writer method so that it shows up in the RECORD?
            fs::copy(&artifact, &bin_path).context(format!(
                "Failed to copy {} to {}",
                artifact.display(),
                bin_path.display()
            ))?;
        }
        BridgeModel::Cffi => {
            let artifact = build_context.compile_cdylib(None, None).context(context)?;

            // Uninstall the old code
            writer.delete_dir(&build_context.module_name)?;

            write_cffi_module(
                &mut writer,
                &build_context.project_layout,
                build_context.manifest_path.parent().unwrap(),
                &build_context.module_name,
                &artifact,
                &interpreter.executable,
                true,
            )?;
        }
        BridgeModel::Bindings(_) => {
            let artifact = build_context
                .compile_cdylib(Some(&interpreter), Some(&build_context.module_name))
                .context(context)?;

            // Uninstall the old code
            writer.delete_dir(&build_context.module_name)?;

            write_bindings_module(
                &mut writer,
                &build_context.project_layout,
                &build_context.module_name,
                &artifact,
                Some(&interpreter),
                &target,
                true,
            )?;
        }
        BridgeModel::BindingsAbi3(_, _) => {
            let artifact = build_context
                // We need the interpreter on windows
                .compile_cdylib(Some(&interpreter), Some(&build_context.module_name))
                .context(context)?;

            // Uninstall the old code
            writer.delete_dir(&build_context.module_name)?;

            write_bindings_module(
                &mut writer,
                &build_context.project_layout,
                &build_context.module_name,
                &artifact,
                None,
                &target,
                true,
            )?;
        }
    }

    // Write dist-info directory so pip can interact with it
    // We skip running auditwheel and simply tag as linux
    let tags = match build_context.bridge {
        BridgeModel::Bindings(_) => {
            vec![build_context.interpreter[0].get_tag(PlatformTag::Linux, build_context.universal2)]
        }
        BridgeModel::BindingsAbi3(major, minor) => {
            let platform = target.get_platform_tag(PlatformTag::Linux, build_context.universal2);
            vec![format!("cp{}{}-abi3-{}", major, minor, platform)]
        }
        BridgeModel::Bin | BridgeModel::Cffi => {
            build_context
                .target
                .get_universal_tags(PlatformTag::Linux, build_context.universal2)
                .1
        }
    };

    write_dist_info(&mut writer, &build_context.metadata21, &tags)?;

    // https://packaging.python.org/specifications/recording-installed-packages/#the-installer-file
    writer.add_bytes(
        build_context
            .metadata21
            .get_dist_info_dir()
            .join("INSTALLER"),
        env!("CARGO_PKG_NAME").as_bytes(),
    )?;

    writer.write_record(&build_context.metadata21)?;

    write_entry_points(&interpreter, &build_context.metadata21)?;

    Ok(())
}

/// https://packaging.python.org/specifications/entry-points/
///
/// entry points examples:
/// 1. `foomod:main`
/// 2. `foomod:main_bar [bar,baz]` where `bar` and `baz` are extra requires
fn parse_entry_point(entry: &str) -> Option<(&str, &str)> {
    // remove extras since we don't care about them
    let entry = entry
        .split_once(' ')
        .map(|(first, _)| first)
        .unwrap_or(entry);
    entry.split_once(':')
}

/// Build a shebang line. In the simple case (on Windows, or a shebang line
/// which is not too long or contains spaces) use a simple formulation for
/// the shebang. Otherwise, use /bin/sh as the executable, with a contrived
/// shebang which allows the script to run either under Python or sh, using
/// suitable quoting. Thanks to Harald Nordgren for his input.
/// See also: http://www.in-ulm.de/~mascheck/various/shebang/#length
///           https://hg.mozilla.org/mozilla-central/file/tip/mach
fn get_shebang(executable: &Path) -> String {
    let executable = executable.display().to_string();
    if cfg!(unix) {
        let max_length = if cfg!(target_os = "macos") { 512 } else { 127 };
        // Add 3 for '#!' prefix and newline suffix.
        let shebang_length = executable.len() + 3;
        if !executable.contains(' ') && shebang_length <= max_length {
            return format!("#!{}\n", executable);
        }
        let mut shebang = "#!/bin/sh\n".to_string();
        shebang.push_str(&format!("'''exec' {} \"$0\" \"$@\"\n' '''", executable));
        shebang
    } else {
        format!("#!{}\n", executable)
    }
}

#[cfg(target_os = "windows")]
fn get_launcher(interpreter: &PythonInterpreter, gui: bool) -> Result<&[u8]> {
    let pointer_size =
        interpreter.run_script("import struct; print(struct.calcsize('P'), end='')")?;
    let platform = interpreter
        .run_script("from sysconfig import get_platform; print(get_platform(), end='')")?;
    let launcher = match (pointer_size.trim(), platform.as_str(), gui) {
        ("8", "win-amd64", true) => WIN_LAUNCHER_W64,
        ("4", "win-amd64", true) => WIN_LAUNCHER_W32,
        ("8", "win-amd64", false) => WIN_LAUNCHER_T64,
        ("4", "win-amd64", false) => WIN_LAUNCHER_T32,
        ("8", "win-arm64", true) => WIN_LAUNCHER_W64_ARM,
        ("8", "win-arm64", false) => WIN_LAUNCHER_T64_ARM,
        (_, _, _) => bail!("unsupported python interpreter"),
    };
    Ok(launcher)
}

fn write_entry_points(interpreter: &PythonInterpreter, metadata21: &Metadata21) -> Result<()> {
    let code = "import sysconfig; print(sysconfig.get_path('scripts'))";
    let script_dir = interpreter.run_script(code)?;
    let script_dir = Path::new(script_dir.trim());
    let shebang = get_shebang(&interpreter.executable);
    for (name, entry, _gui) in metadata21
        .scripts
        .iter()
        .map(|(name, entry)| (name, entry, false))
        .chain(
            metadata21
                .gui_scripts
                .iter()
                .map(|(name, entry)| (name, entry, true)),
        )
    {
        let (module, func) =
            parse_entry_point(entry).context("Invalid entry point specification")?;
        let import_name = func.split_once('.').map(|(first, _)| first).unwrap_or(func);
        let script = format!(
            r#"# -*- coding: utf-8 -*-
import re
import sys
from {module} import {import_name}
if __name__ == '__main__':
    sys.argv[0] = re.sub(r'(-script\.pyw|\.exe)?$', '', sys.argv[0])
    sys.exit({func}())
"#,
            module = module,
            import_name = import_name,
            func = func,
        );
        // Only support launch scripts with PEP 397 launcher on Windows now
        let ext = interpreter
            .run_script("import sysconfig; print(sysconfig.get_config_var('EXE'), end='')")?;
        let script_path = script_dir.join(format!("{}{}", name, ext));
        // We only need to set the executable bit on unix
        let mut file = {
            #[cfg(not(target_os = "windows"))]
            {
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .mode(0o755)
                    .open(&script_path)
            }
            #[cfg(target_os = "windows")]
            {
                fs::File::create(&script_path)
            }
        }
        .context(format!(
            "Failed to create a file at {}",
            script_path.display()
        ))?;

        let mut write_all = |bytes: &[u8]| -> Result<()> {
            file.write_all(bytes).context(format!(
                "Failed to write to file at {}",
                script_path.display()
            ))
        };

        #[cfg(target_os = "windows")]
        {
            let launcher = get_launcher(interpreter, _gui)?;
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            zip.start_file("__main__.py", zip::write::FileOptions::default())?;
            zip.write_all(script.as_bytes())?;
            let archive = zip.finish()?;
            write_all(launcher)?;
            write_all(shebang.as_bytes())?;
            write_all(&archive.into_inner())?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            let script = shebang.clone() + &script;
            write_all(script.as_bytes())?;
        }
    }

    Ok(())
}
