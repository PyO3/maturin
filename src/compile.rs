use crate::build_context::BridgeModel;
use crate::target::RUST_1_64_0;
#[cfg(feature = "zig")]
use crate::PlatformTag;
use crate::{BuildContext, PythonInterpreter, Target};
use anyhow::{anyhow, bail, Context, Result};
use fat_macho::FatWriter;
use fs_err::{self as fs, File};
use std::collections::HashMap;
use std::env;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str;
use tracing::{debug, trace};

/// The first version of pyo3 that supports building Windows abi3 wheel
/// without `PYO3_NO_PYTHON` environment variable
const PYO3_ABI3_NO_PYTHON_VERSION: (u64, u64, u64) = (0, 16, 4);

/// crate types excluding `bin`, `cdylib` and `proc-macro`
pub(crate) const LIB_CRATE_TYPES: [&str; 4] = ["lib", "dylib", "rlib", "staticlib"];

pub(crate) type CompileTarget = (cargo_metadata::Target, BridgeModel);

/// A cargo build artifact
#[derive(Debug, Clone)]
pub struct BuildArtifact {
    /// Path to the build artifact
    pub path: PathBuf,
    /// Array of paths to include in the library search path, as indicated by
    /// the `cargo:rustc-link-search` instruction.
    pub linked_paths: Vec<String>,
}

/// Builds the rust crate into a native module (i.e. an .so or .dll) for a
/// specific python version. Returns a mapping from crate type (e.g. cdylib)
/// to artifact location.
pub fn compile(
    context: &BuildContext,
    python_interpreter: Option<&PythonInterpreter>,
    targets: &[CompileTarget],
) -> Result<Vec<HashMap<String, BuildArtifact>>> {
    if context.target.is_macos() && context.universal2 {
        compile_universal2(context, python_interpreter, targets)
    } else {
        compile_targets(context, python_interpreter, targets)
    }
}

/// Build an universal2 wheel for macos which contains both an x86 and an aarch64 binary
fn compile_universal2(
    context: &BuildContext,
    python_interpreter: Option<&PythonInterpreter>,
    targets: &[CompileTarget],
) -> Result<Vec<HashMap<String, BuildArtifact>>> {
    let mut aarch64_context = context.clone();
    aarch64_context.target = Target::from_target_triple(Some("aarch64-apple-darwin".to_string()))?;

    let aarch64_artifacts = compile_targets(&aarch64_context, python_interpreter, targets)
        .context("Failed to build a aarch64 library through cargo")?;
    let mut x86_64_context = context.clone();
    x86_64_context.target = Target::from_target_triple(Some("x86_64-apple-darwin".to_string()))?;

    let x86_64_artifacts = compile_targets(&x86_64_context, python_interpreter, targets)
        .context("Failed to build a x86_64 library through cargo")?;

    let mut universal_artifacts = Vec::with_capacity(targets.len());
    for (bridge_model, (aarch64_artifact, x86_64_artifact)) in targets
        .iter()
        .map(|(_, bridge_model)| bridge_model)
        .zip(aarch64_artifacts.iter().zip(&x86_64_artifacts))
    {
        let build_type = if bridge_model.is_bin() {
            "bin"
        } else {
            "cdylib"
        };
        let aarch64_artifact = aarch64_artifact.get(build_type).cloned().ok_or_else(|| {
            if build_type == "cdylib" {
                anyhow!(
                    "Cargo didn't build an aarch64 cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?",
                )
            } else {
                anyhow!("Cargo didn't build an aarch64 bin.")
            }
        })?;
        let x86_64_artifact = x86_64_artifact.get(build_type).cloned().ok_or_else(|| {
            if build_type == "cdylib" {
                anyhow!(
                    "Cargo didn't build a x86_64 cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?",
                )
            } else {
                anyhow!("Cargo didn't build a x86_64 bin.")
            }
        })?;
        // Create an universal dylib
        let output_path = aarch64_artifact
            .path
            .display()
            .to_string()
            .replace("aarch64-apple-darwin/", "");
        let mut writer = FatWriter::new();
        let aarch64_file = fs::read(&aarch64_artifact.path)?;
        let x86_64_file = fs::read(&x86_64_artifact.path)?;
        writer
            .add(aarch64_file)
            .map_err(|e| anyhow!("Failed to add aarch64 cdylib: {:?}", e))?;
        writer
            .add(x86_64_file)
            .map_err(|e| anyhow!("Failed to add x86_64 cdylib: {:?}", e))?;
        writer
            .write_to_file(&output_path)
            .map_err(|e| anyhow!("Failed to create universal cdylib: {:?}", e))?;

        let mut result = HashMap::new();
        let universal_artifact = BuildArtifact {
            path: PathBuf::from(output_path),
            ..x86_64_artifact
        };
        result.insert(build_type.to_string(), universal_artifact);
        universal_artifacts.push(result);
    }
    Ok(universal_artifacts)
}

fn compile_targets(
    context: &BuildContext,
    python_interpreter: Option<&PythonInterpreter>,
    targets: &[CompileTarget],
) -> Result<Vec<HashMap<String, BuildArtifact>>> {
    let mut artifacts = Vec::with_capacity(targets.len());
    for (target, bridge_model) in targets {
        artifacts.push(compile_target(
            context,
            python_interpreter,
            bridge_model,
            target,
        )?);
    }
    Ok(artifacts)
}

fn compile_target(
    context: &BuildContext,
    python_interpreter: Option<&PythonInterpreter>,
    bridge_model: &BridgeModel,
    binding_target: &cargo_metadata::Target,
) -> Result<HashMap<String, BuildArtifact>> {
    let target = &context.target;

    let mut cargo_rustc: cargo_options::Rustc = context.cargo_options.clone().into();
    cargo_rustc.message_format = vec!["json".to_string()];

    // --release and --profile are conflicting options
    if context.release && cargo_rustc.profile.is_none() {
        cargo_rustc.release = true;
    }

    // Add `--crate-type cdylib` if available
    if binding_target
        .kind
        .iter()
        .any(|k| LIB_CRATE_TYPES.contains(&k.as_str()))
    {
        // `--crate-type` is stable since Rust 1.64.0
        // See https://github.com/rust-lang/cargo/pull/10838
        if target.rustc_version.semver >= RUST_1_64_0 {
            debug!("Setting crate_type to cdylib for Rust >= 1.64.0");
            cargo_rustc.crate_type = vec!["cdylib".to_string()];
        }
    }

    let target_triple = target.target_triple();

    let manifest_dir = context.manifest_path.parent().unwrap();
    let mut rustflags = cargo_config2::Config::load_with_cwd(manifest_dir)?
        .rustflags(target_triple)?
        .unwrap_or_default();

    // We need to pass --bin / --lib
    match bridge_model {
        BridgeModel::Bin(..) => {
            cargo_rustc.bin.push(binding_target.name.clone());
        }
        BridgeModel::Cffi
        | BridgeModel::UniFfi
        | BridgeModel::Bindings(..)
        | BridgeModel::BindingsAbi3(..) => {
            cargo_rustc.lib = true;
            // https://github.com/rust-lang/rust/issues/59302#issue-422994250
            // We must only do this for libraries as it breaks binaries
            // For some reason this value is ignored when passed as rustc argument
            if context.target.is_musl_target()
                && !rustflags
                    .flags
                    .iter()
                    .any(|f| f == "target-feature=-crt-static")
            {
                debug!("Setting `-C target-features=-crt-static` for musl dylib");
                rustflags.push("-C");
                rustflags.push("target-feature=-crt-static");
            }
        }
    }

    // https://github.com/PyO3/pyo3/issues/88#issuecomment-337744403
    if target.is_macos() {
        if let BridgeModel::Bindings(..) | BridgeModel::BindingsAbi3(..) = bridge_model {
            // Change LC_ID_DYLIB to the final .so name for macOS targets to avoid linking with
            // non-existent library.
            // See https://github.com/PyO3/setuptools-rust/issues/106 for detail
            let module_name = &context.module_name;
            let so_filename = match bridge_model {
                BridgeModel::BindingsAbi3(..) => format!("{module_name}.abi3.so"),
                _ => python_interpreter
                    .expect("missing python interpreter for non-abi3 wheel build")
                    .get_library_name(module_name),
            };
            let macos_dylib_install_name =
                format!("link-args=-Wl,-install_name,@rpath/{so_filename}");
            let mac_args = [
                "-C".to_string(),
                "link-arg=-undefined".to_string(),
                "-C".to_string(),
                "link-arg=dynamic_lookup".to_string(),
                "-C".to_string(),
                macos_dylib_install_name,
            ];
            debug!("Setting additional linker args for macOS: {:?}", mac_args);
            cargo_rustc.args.extend(mac_args);
        }
    } else if target.is_emscripten() {
        // Allow user to override these default flags
        if !rustflags.flags.iter().any(|f| f == "link-native-libraries") {
            debug!("Setting `-Z link-native-libraries=no` for Emscripten");
            rustflags.push("-Z");
            rustflags.push("link-native-libraries=no");
        }
        let mut emscripten_args = Vec::new();
        // Allow user to override these default settings
        if !cargo_rustc
            .args
            .iter()
            .any(|arg| arg.contains("SIDE_MODULE"))
        {
            emscripten_args.push("-C".to_string());
            emscripten_args.push("link-arg=-sSIDE_MODULE=2".to_string());
        }
        if !cargo_rustc
            .args
            .iter()
            .any(|arg| arg.contains("WASM_BIGINT"))
        {
            emscripten_args.push("-C".to_string());
            emscripten_args.push("link-arg=-sWASM_BIGINT".to_string());
        }
        debug!(
            "Setting additional linker args for Emscripten: {:?}",
            emscripten_args
        );
        cargo_rustc.args.extend(emscripten_args);
    }

    if context.strip {
        cargo_rustc
            .args
            .extend(["-C".to_string(), "link-arg=-s".to_string()]);
    }

    let mut build_command = if target.is_msvc() && target.cross_compiling() {
        #[cfg(feature = "xwin")]
        {
            let mut build = cargo_xwin::Rustc::from(cargo_rustc);

            build.target = vec![target_triple.to_string()];
            build.build_command()?
        }
        #[cfg(not(feature = "xwin"))]
        {
            if target.user_specified {
                cargo_rustc.target = vec![target_triple.to_string()];
            }
            cargo_rustc.command()
        }
    } else {
        #[cfg(feature = "zig")]
        {
            let mut build = cargo_zigbuild::Rustc::from(cargo_rustc);
            if !context.zig {
                build.disable_zig_linker = true;
                if target.user_specified {
                    build.target = vec![target_triple.to_string()];
                }
            } else {
                build.enable_zig_ar = true;
                let zig_triple = if target.is_linux() && !target.is_musl_target() {
                    match context.platform_tag.iter().find(|tag| tag.is_manylinux()) {
                        Some(PlatformTag::Manylinux { x, y }) => {
                            format!("{target_triple}.{x}.{y}")
                        }
                        _ => target_triple.to_string(),
                    }
                } else {
                    target_triple.to_string()
                };
                build.target = vec![zig_triple];
            }
            build.build_command()?
        }
        #[cfg(not(feature = "zig"))]
        {
            if target.user_specified {
                cargo_rustc.target = vec![target_triple.to_string()];
            }
            cargo_rustc.command()
        }
    };

    #[cfg(feature = "zig")]
    if context.zig {
        // Pass zig command to downstream, eg. python3-dll-a
        if let Ok((zig_cmd, zig_args)) = cargo_zigbuild::Zig::find_zig() {
            if zig_args.is_empty() {
                build_command.env("ZIG_COMMAND", zig_cmd);
            } else {
                build_command.env(
                    "ZIG_COMMAND",
                    format!("{} {}", zig_cmd.display(), zig_args.join(" ")),
                );
            };
        }
    }

    build_command
        // We need to capture the json messages
        .stdout(Stdio::piped())
        // We can't get colored human and json messages from rustc as they are mutually exclusive,
        // but forwarding stderr is still useful in case there some non-json error
        .stderr(Stdio::inherit());

    if !rustflags.flags.is_empty() {
        build_command.env("CARGO_ENCODED_RUSTFLAGS", rustflags.encode()?);
    }

    if let BridgeModel::BindingsAbi3(_, _) = bridge_model {
        let is_pypy = python_interpreter
            .map(|p| p.interpreter_kind.is_pypy())
            .unwrap_or(false);
        if !is_pypy && !target.is_windows() {
            let pyo3_ver = pyo3_version(&context.cargo_metadata)
                .context("Failed to get pyo3 version from cargo metadata")?;
            if pyo3_ver < PYO3_ABI3_NO_PYTHON_VERSION {
                // This will make old pyo3's build script only set some predefined linker
                // arguments without trying to read any python configuration
                build_command.env("PYO3_NO_PYTHON", "1");
            }
        }
    }

    // Setup `PYO3_CONFIG_FILE` if we are cross compiling for pyo3 bindings
    if let Some(interpreter) = python_interpreter {
        // Target python interpreter isn't runnable when cross compiling
        if interpreter.runnable {
            if bridge_model.is_bindings("pyo3")
                || bridge_model.is_bindings("pyo3-ffi")
                || (matches!(bridge_model, BridgeModel::BindingsAbi3(_, _))
                    && interpreter.interpreter_kind.is_pypy())
            {
                build_command
                    .env("PYO3_PYTHON", &interpreter.executable)
                    .env(
                        "PYO3_ENVIRONMENT_SIGNATURE",
                        interpreter.environment_signature(),
                    );
            }

            // rust-cpython, and legacy pyo3 versions
            build_command.env("PYTHON_SYS_EXECUTABLE", &interpreter.executable);
        } else if (bridge_model.is_bindings("pyo3")
            || bridge_model.is_bindings("pyo3-ffi")
            || (matches!(bridge_model, BridgeModel::BindingsAbi3(_, _))
                && interpreter.interpreter_kind.is_pypy()))
            && env::var_os("PYO3_CONFIG_FILE").is_none()
        {
            let pyo3_config = interpreter.pyo3_config_file();
            let maturin_target_dir = context.target_dir.join("maturin");
            let config_file = maturin_target_dir.join(format!(
                "pyo3-config-{}-{}.{}.txt",
                target_triple, interpreter.major, interpreter.minor
            ));
            fs::create_dir_all(&maturin_target_dir)?;
            fs::write(&config_file, pyo3_config).with_context(|| {
                format!(
                    "Failed to create pyo3 config file at '{}'",
                    config_file.display()
                )
            })?;
            build_command.env("PYO3_CONFIG_FILE", config_file);
        }
    }

    if let Some(lib_dir) = env::var_os("MATURIN_PYTHON_SYSCONFIGDATA_DIR") {
        build_command.env("PYO3_CROSS_LIB_DIR", lib_dir);
    }

    // Set default macOS deployment target version
    if target.is_macos() && env::var_os("MACOSX_DEPLOYMENT_TARGET").is_none() {
        use crate::target::rustc_macosx_target_version;

        let (major, minor) = rustc_macosx_target_version(target_triple);
        build_command.env("MACOSX_DEPLOYMENT_TARGET", format!("{major}.{minor}"));
        eprintln!(
            "💻 Using `MACOSX_DEPLOYMENT_TARGET={major}.{minor}` for {target_triple} by default"
        );
    }

    debug!("Running {:?}", build_command);

    let mut cargo_build = build_command
        .spawn()
        .context("Failed to run `cargo rustc`")?;

    let mut artifacts = HashMap::new();
    let mut linked_paths = Vec::new();

    let stream = cargo_build
        .stdout
        .take()
        .expect("Cargo build should have a stdout");
    for message in cargo_metadata::Message::parse_stream(BufReader::new(stream)) {
        let message = message.context("Failed to parse cargo metadata message")?;
        trace!("cargo message: {:?}", message);
        match message {
            cargo_metadata::Message::CompilerArtifact(artifact) => {
                let package_in_metadata = context
                    .cargo_metadata
                    .packages
                    .iter()
                    .find(|package| package.id == artifact.package_id);
                let crate_name = match package_in_metadata {
                    Some(package) => &package.name,
                    None => {
                        let package_id = &artifact.package_id;
                        // Ignore the package if it's coming from Rust sysroot when compiling with `-Zbuild-std`
                        let should_warn = !package_id.repr.contains("rustup")
                            && !package_id.repr.contains("rustlib")
                            && !artifact.features.contains(&"rustc-dep-of-std".to_string());
                        if should_warn {
                            // This is a spurious error I don't really understand
                            eprintln!(
                                "⚠️  Warning: The package {package_id} wasn't listed in `cargo metadata`"
                            );
                        }
                        continue;
                    }
                };

                // Extract the location of the .so/.dll/etc. from cargo's json output
                if crate_name == &context.crate_name {
                    let tuples = artifact
                        .target
                        .crate_types
                        .into_iter()
                        .zip(artifact.filenames);
                    for (crate_type, filename) in tuples {
                        let artifact = BuildArtifact {
                            path: filename.into(),
                            linked_paths: Vec::new(),
                        };
                        artifacts.insert(crate_type, artifact);
                    }
                }
            }
            // See https://doc.rust-lang.org/cargo/reference/external-tools.html#build-script-output
            cargo_metadata::Message::BuildScriptExecuted(msg) => {
                for path in msg.linked_paths.iter().map(|p| p.as_str()) {
                    // `linked_paths` may include a "KIND=" prefix in the string where KIND is the library kind
                    if let Some(index) = path.find('=') {
                        linked_paths.push(path[index + 1..].to_string());
                    } else {
                        linked_paths.push(path.to_string());
                    }
                }
            }
            cargo_metadata::Message::CompilerMessage(msg) => {
                println!("{}", msg.message);
            }
            _ => (),
        }
    }

    // Add linked_paths to build artifacts
    for artifact in artifacts.values_mut() {
        artifact.linked_paths = linked_paths.clone();
    }

    let status = cargo_build
        .wait()
        .expect("Failed to wait on cargo child process");

    if !status.success() {
        bail!(
            r#"Cargo build finished with "{}": `{:?}`"#,
            status,
            build_command,
        )
    }

    Ok(artifacts)
}

/// Checks that the native library contains a function called `PyInit_<module name>` and warns
/// if it's missing.
///
/// That function is the python's entrypoint for loading native extensions, i.e. python will fail
/// to import the module with error if it's missing or named incorrectly
///
/// Currently the check is only run on linux, macOS and Windows
pub fn warn_missing_py_init(artifact: &Path, module_name: &str) -> Result<()> {
    let py_init = format!("PyInit_{module_name}");
    let mut fd = File::open(artifact)?;
    let mut buffer = Vec::new();
    fd.read_to_end(&mut buffer)?;
    let mut found = false;
    match goblin::Object::parse(&buffer)? {
        goblin::Object::Elf(elf) => {
            for dyn_sym in elf.dynsyms.iter() {
                if py_init == elf.dynstrtab[dyn_sym.st_name] {
                    found = true;
                    break;
                }
            }
        }
        goblin::Object::Mach(mach) => {
            match mach {
                goblin::mach::Mach::Binary(macho) => {
                    for sym in macho.exports()? {
                        let sym_name = sym.name;
                        if py_init == sym_name.strip_prefix('_').unwrap_or(&sym_name) {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        for sym in macho.symbols() {
                            let (sym_name, _) = sym?;
                            if py_init == sym_name.strip_prefix('_').unwrap_or(sym_name) {
                                found = true;
                                break;
                            }
                        }
                    }
                }
                goblin::mach::Mach::Fat(_) => {
                    // Ignore fat macho,
                    // we only generate them by combining thin binaries which is handled above
                    found = true
                }
            }
        }
        goblin::Object::PE(pe) => {
            for sym in &pe.exports {
                if let Some(sym_name) = sym.name {
                    if py_init == sym_name {
                        found = true;
                        break;
                    }
                }
            }
        }
        _ => {
            // Currently, only linux, macOS and Windows are implemented
            found = true
        }
    }

    if !found {
        eprintln!(
            "⚠️  Warning: Couldn't find the symbol `{py_init}` in the native library. \
             Python will fail to import this module. \
             If you're using pyo3, check that `#[pymodule]` uses `{module_name}` as module name"
        )
    }

    Ok(())
}

fn pyo3_version(cargo_metadata: &cargo_metadata::Metadata) -> Option<(u64, u64, u64)> {
    let packages: HashMap<&str, &cargo_metadata::Package> = cargo_metadata
        .packages
        .iter()
        .filter_map(|pkg| {
            let name = &pkg.name;
            if name == "pyo3" || name == "pyo3-ffi" {
                Some((name.as_ref(), pkg))
            } else {
                None
            }
        })
        .collect();
    packages
        .get("pyo3")
        .or_else(|| packages.get("pyo3-ffi"))
        .map(|pkg| (pkg.version.major, pkg.version.minor, pkg.version.patch))
}
