use crate::auditwheel::PlatformTag;
use crate::build_context::{BridgeModel, ProjectLayout};
use crate::cross_compile::{find_sysconfigdata, parse_sysconfigdata};
use crate::python_interpreter::{InterpreterKind, MINIMUM_PYTHON_MINOR};
use crate::BuildContext;
use crate::CargoToml;
use crate::Metadata21;
use crate::PyProjectToml;
use crate::PythonInterpreter;
use crate::Target;
use anyhow::{bail, format_err, Context, Result};
use cargo_metadata::{Metadata, MetadataCommand, Node};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::io;
use std::path::PathBuf;

// This is used for BridgeModel::Bindings("pyo3-ffi") and BridgeModel::Bindings("pyo3").
// These should be treated almost identically but must be correctly identified
// as one or the other in logs. pyo3-ffi is ordered first because it is newer
// and more restrictive.
const PYO3_BINDING_CRATES: [&str; 2] = ["pyo3-ffi", "pyo3"];

fn pyo3_minimum_python_minor_version(major_version: u64, minor_version: u64) -> Option<usize> {
    if (major_version, minor_version) >= (0, 16) {
        Some(7)
    } else if (major_version, minor_version) >= (0, 12) {
        Some(6)
    } else {
        None
    }
}

fn pyo3_ffi_minimum_python_minor_version(major_version: u64, minor_version: u64) -> Option<usize> {
    if (major_version, minor_version) >= (0, 16) {
        pyo3_minimum_python_minor_version(major_version, minor_version)
    } else {
        None
    }
}

/// High level API for building wheels from a crate which is also used for the CLI
#[derive(Debug, Default, Serialize, Deserialize, clap::Parser, Clone, Eq, PartialEq)]
#[serde(default)]
pub struct BuildOptions {
    /// Control the platform tag on linux.
    ///
    /// Options are `manylinux` tags (for example `manylinux2014`/`manylinux_2_24`)
    /// or `musllinux` tags (for example `musllinux_1_2`)
    /// and `linux` for the native linux tag.
    ///
    /// Note that `manylinux1` is unsupported by the rust compiler. Wheels with the native `linux` tag
    /// will be rejected by pypi, unless they are separately validated by `auditwheel`.
    ///
    /// The default is the lowest compatible `manylinux` tag, or plain `linux` if nothing matched
    ///
    /// This option is ignored on all non-linux platforms
    #[clap(
        name = "compatibility",
        long = "compatibility",
        alias = "manylinux",
        parse(try_from_str)
    )]
    pub platform_tag: Option<PlatformTag>,

    /// The python versions to build wheels for, given as the names of the
    /// interpreters. Uses autodiscovery if not explicitly set.
    #[clap(short, long, multiple_values = true, multiple_occurrences = true)]
    pub interpreter: Vec<PathBuf>,

    /// Which kind of bindings to use. Possible values are pyo3, rust-cpython, cffi and bin
    #[clap(short, long)]
    pub bindings: Option<String>,

    /// The path to the Cargo.toml
    #[clap(short = 'm', long = "manifest-path", parse(from_os_str), name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// The directory to store the built wheels in. Defaults to a new "wheels"
    /// directory in the project's target directory
    #[clap(short, long, parse(from_os_str))]
    pub out: Option<PathBuf>,

    /// Don't check for manylinux compliance
    #[clap(long = "skip-auditwheel")]
    pub skip_auditwheel: bool,

    /// For manylinux targets, use zig to ensure compliance for the chosen manylinux version
    ///
    /// Default to manylinux2010/manylinux_2_12 if you do not specify an `--compatibility`
    ///
    /// Make sure you installed zig with `pip install maturin[zig]`
    #[clap(long)]
    pub zig: bool,

    /// The --target option for cargo
    #[clap(long, name = "TRIPLE", env = "CARGO_BUILD_TARGET")]
    pub target: Option<String>,

    /// Extra arguments that will be passed to cargo as `cargo rustc [...] [arg1] [arg2] -- [...]`
    ///
    /// Use as `--cargo-extra-args="--my-arg"`
    ///
    /// Note that maturin invokes cargo twice: Once as `cargo metadata` and then as `cargo rustc`.
    /// maturin tries to pass only the shared subset of options to cargo metadata, but this is may
    /// be a bit flaky.
    #[clap(long = "cargo-extra-args")]
    pub cargo_extra_args: Vec<String>,

    /// Extra arguments that will be passed to rustc as `cargo rustc [...] -- [...] [arg1] [arg2]`
    ///
    /// Use as `--rustc-extra-args="--my-arg"`
    #[clap(long = "rustc-extra-args")]
    pub rustc_extra_args: Vec<String>,

    /// Control whether to build universal2 wheel for macOS or not.
    /// Only applies to macOS targets, do nothing otherwise.
    #[clap(long)]
    pub universal2: bool,
}

impl BuildOptions {
    /// Get cargo manifest file path
    fn manifest_path(&self) -> Result<PathBuf> {
        // use command line argument if specified
        if let Some(path) = &self.manifest_path {
            return Ok(path.clone());
        }
        // check `manifest-path` option in pyproject.toml
        let current_dir = env::current_dir()
            .context("Failed to detect current directory ಠ_ಠ")?
            .canonicalize()?;
        if current_dir.join("pyproject.toml").is_file() {
            let pyproject =
                PyProjectToml::new(&current_dir).context("pyproject.toml is invalid")?;
            if let Some(path) = pyproject.manifest_path() {
                println!("🔗 Found cargo manifest path in pyproject.toml");
                // pyproject.toml must be placed at top directory
                let manifest_dir = path
                    .parent()
                    .context("missing parent directory")?
                    .canonicalize()?;
                if !manifest_dir.starts_with(&current_dir) {
                    bail!("Cargo.toml can not be placed outside of the directory containing pyproject.toml");
                }
                return Ok(path.to_path_buf());
            }
        }
        // check Cargo.toml in current directory
        let path = PathBuf::from("Cargo.toml");
        if path.exists() {
            Ok(path)
        } else {
            Err(format_err!(
                "Can't find {} (in {})",
                path.display(),
                current_dir.display()
            ))
        }
    }
    /// Tries to fill the missing metadata for a BuildContext by querying cargo and python
    pub fn into_build_context(
        self,
        release: bool,
        strip: bool,
        editable: bool,
    ) -> Result<BuildContext> {
        let manifest_file = self.manifest_path()?;
        if !manifest_file.is_file() {
            bail!(
                "{} is not the path to a Cargo.toml",
                manifest_file.display()
            );
        }

        let cargo_toml = CargoToml::from_path(&manifest_file)?;
        let manifest_dir = manifest_file.parent().unwrap();
        let pyproject: Option<PyProjectToml> = if manifest_dir.join("pyproject.toml").is_file() {
            let pyproject =
                PyProjectToml::new(manifest_dir).context("pyproject.toml is invalid")?;
            pyproject.warn_missing_maturin_version();
            pyproject.warn_missing_build_backend();
            Some(pyproject)
        } else {
            None
        };
        let pyproject = pyproject.as_ref();
        let metadata21 = Metadata21::from_cargo_toml(&cargo_toml, &manifest_dir)
            .context("Failed to parse Cargo.toml into python metadata")?;
        let extra_metadata = cargo_toml.remaining_core_metadata();

        let crate_name = &cargo_toml.package.name;

        // If the package name contains minuses, you must declare a module with
        // underscores as lib name
        let module_name = cargo_toml
            .lib
            .as_ref()
            .and_then(|lib| lib.name.as_ref())
            .unwrap_or(crate_name)
            .to_owned();

        // Only use extension name from extra metadata if it contains dot
        let extension_name = extra_metadata
            .name
            .as_ref()
            .filter(|name| name.contains('.'))
            .unwrap_or(&module_name);

        let project_layout = ProjectLayout::determine(
            manifest_dir,
            extension_name,
            extra_metadata.python_source.as_deref(),
        )?;

        let mut args_from_pyproject = Vec::new();
        let mut cargo_extra_args = self.cargo_extra_args.clone();
        if cargo_extra_args.is_empty() {
            // if not supplied on command line, try pyproject.toml
            if let Some(args) = pyproject.and_then(|x| x.cargo_extra_args()) {
                cargo_extra_args.push(args.to_string());
                args_from_pyproject.push("cargo-extra-args");
            }
        }
        cargo_extra_args = split_extra_args(&cargo_extra_args)?;

        let cargo_metadata_extra_args = extract_cargo_metadata_args(&cargo_extra_args)?;

        let result = MetadataCommand::new()
            .manifest_path(&manifest_file)
            .other_options(cargo_metadata_extra_args)
            .exec();

        let cargo_metadata = match result {
            Ok(cargo_metadata) => cargo_metadata,
            Err(cargo_metadata::Error::Io(inner)) if inner.kind() == io::ErrorKind::NotFound => {
                // NotFound is the specific error when cargo is not in PATH
                return Err(inner)
                    .context("Cargo metadata failed. Do you have cargo in your PATH?");
            }
            Err(err) => {
                return Err(err)
                    .context("Cargo metadata failed. Does your crate compile with `cargo build`?");
            }
        };

        let bridge = find_bridge(
            &cargo_metadata,
            self.bindings.as_deref().or_else(|| {
                pyproject.and_then(|x| {
                    if x.bindings().is_some() {
                        args_from_pyproject.push("bindings");
                    }
                    x.bindings()
                })
            }),
        )?;

        if bridge != BridgeModel::Bin && module_name.contains('-') {
            bail!(
                "The module name must not contains a minus \
                 (Make sure you have set an appropriate [lib] name in your Cargo.toml)"
            );
        }

        let target = Target::from_target_triple(self.target.clone())?;

        let wheel_dir = match self.out {
            Some(ref dir) => dir.clone(),
            None => PathBuf::from(&cargo_metadata.target_directory).join("wheels"),
        };

        let generate_abi3_import_lib = is_generating_abi3_import_lib(&cargo_metadata)?;
        let interpreter = if self.interpreter.is_empty() {
            // Auto-detect interpreters
            find_interpreter(
                &bridge,
                &[],
                &target,
                get_min_python_minor(&metadata21),
                generate_abi3_import_lib,
            )?
        } else {
            // User given list of interpreters
            find_interpreter(
                &bridge,
                &self.interpreter,
                &target,
                None,
                generate_abi3_import_lib,
            )?
        };

        let mut rustc_extra_args = self.rustc_extra_args.clone();
        if rustc_extra_args.is_empty() {
            // if not supplied on command line, try pyproject.toml
            if let Some(args) = pyproject.and_then(|x| x.rustc_extra_args()) {
                rustc_extra_args.push(args.to_string());
                args_from_pyproject.push("rustc-extra-args");
            }
        }
        rustc_extra_args = split_extra_args(&rustc_extra_args)?;

        let mut universal2 = self.universal2;
        // Also try to determine universal2 from ARCHFLAGS environment variable
        if let Ok(arch_flags) = env::var("ARCHFLAGS") {
            let arches: HashSet<&str> = arch_flags
                .split("-arch")
                .filter_map(|x| {
                    let x = x.trim();
                    if x.is_empty() {
                        None
                    } else {
                        Some(x)
                    }
                })
                .collect();
            if arches.contains("x86_64") && arches.contains("arm64") {
                universal2 = true;
            }
        };
        let strip = pyproject.map(|x| x.strip()).unwrap_or_default() || strip;
        let skip_auditwheel =
            pyproject.map(|x| x.skip_auditwheel()).unwrap_or_default() || self.skip_auditwheel;
        let platform_tag = self
            .platform_tag
            .or_else(|| {
                pyproject.and_then(|x| {
                    if x.compatibility().is_some() {
                        args_from_pyproject.push("compatibility");
                    }
                    x.compatibility()
                })
            })
            .or(if self.zig {
                if target.is_musl_target() {
                    // Zig bundles musl 1.2
                    Some(PlatformTag::Musllinux { x: 1, y: 2 })
                } else {
                    // With zig we can compile to any glibc version that we want, so we pick the lowest
                    // one supported by the rust compiler
                    Some(target.get_minimum_manylinux_tag())
                }
            } else {
                // Defaults to musllinux_1_2 for musl target if it's not bin bindings
                if target.is_musl_target() && !matches!(bridge, BridgeModel::Bin) {
                    Some(PlatformTag::Musllinux { x: 1, y: 2 })
                } else {
                    None
                }
            });
        if platform_tag == Some(PlatformTag::manylinux1()) {
            eprintln!("⚠️  Warning: manylinux1 is unsupported by the Rust compiler.");
        }

        if !args_from_pyproject.is_empty() {
            eprintln!(
                "📡 Using build options {} from pyproject.toml",
                args_from_pyproject.join(", ")
            );
        }

        let target_dir = cargo_extra_args
            .iter()
            .position(|x| x == "--target-dir")
            .and_then(|i| cargo_extra_args.get(i + 1))
            .map(PathBuf::from)
            .unwrap_or_else(|| cargo_metadata.target_directory.clone().into_std_path_buf());

        Ok(BuildContext {
            target,
            bridge,
            project_layout,
            metadata21,
            crate_name: crate_name.to_string(),
            module_name,
            manifest_path: manifest_file,
            target_dir,
            out: wheel_dir,
            release,
            strip,
            skip_auditwheel,
            zig: self.zig,
            platform_tag,
            cargo_extra_args,
            rustc_extra_args,
            interpreter,
            cargo_metadata,
            universal2,
            editable,
        })
    }
}

/// Uses very simple PEP 440 subset parsing to determine the
/// minimum supported python minor version for interpreter search
fn get_min_python_minor(metadata21: &Metadata21) -> Option<usize> {
    if let Some(requires_python) = &metadata21.requires_python {
        let regex = Regex::new(r#">=3\.(\d+)(?:\.\d)?"#).unwrap();
        if let Some(captures) = regex.captures(requires_python) {
            let min_python_minor = captures[1]
                .parse::<usize>()
                .expect("Regex must only match usize");
            Some(min_python_minor)
        } else {
            println!(
                "⚠️ Couldn't parse the value of requires-python, \
                    not taking it into account when searching for python interpreter. \
                    Note: Only `>=3.x.y` is currently supported."
            );
            None
        }
    } else {
        None
    }
}

/// pyo3 supports building abi3 wheels if the unstable-api feature is not selected
fn has_abi3(cargo_metadata: &Metadata) -> Result<Option<(u8, u8)>> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .context("Expected cargo to return metadata with resolve")?;
    for &lib in PYO3_BINDING_CRATES.iter() {
        let pyo3_packages = resolve
            .nodes
            .iter()
            .filter(|package| cargo_metadata[&package.id].name.as_str() == lib)
            .collect::<Vec<_>>();
        match pyo3_packages.as_slice() {
            [pyo3_crate] => {
                // Find the minimal abi3 python version. If there is none, abi3 hasn't been selected
                // This parser abi3-py{major}{minor} and returns the minimal (major, minor) tuple
                let abi3_selected = pyo3_crate.features.iter().any(|x| x == "abi3");

                let min_abi3_version = pyo3_crate
                    .features
                    .iter()
                    .filter(|x| x.starts_with("abi3-py") && x.len() >= "abi3-pyxx".len())
                    .map(|x| {
                        Ok((
                            (x.as_bytes()[7] as char).to_string().parse::<u8>()?,
                            x[8..].parse::<u8>()?,
                        ))
                    })
                    .collect::<Result<Vec<(u8, u8)>>>()
                    .context(format!("Bogus {} cargo features", lib))?
                    .into_iter()
                    .min();
                if abi3_selected && min_abi3_version.is_none() {
                    bail!(
                        "You have selected the `abi3` feature but not a minimum version (e.g. the `abi3-py36` feature). \
                        maturin needs a minimum version feature to build abi3 wheels."
                    )
                }
                return Ok(min_abi3_version);
            }
            _ => continue,
        }
    }
    Ok(None)
}

/// pyo3 0.16.4+ supports building abi3 wheels without a working Python interpreter for Windows
/// when `generate-abi3-import-lib` feature is enabled
fn is_generating_abi3_import_lib(cargo_metadata: &Metadata) -> Result<bool> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .context("Expected cargo to return metadata with resolve")?;
    for &lib in PYO3_BINDING_CRATES.iter().rev() {
        let pyo3_packages = resolve
            .nodes
            .iter()
            .filter(|package| cargo_metadata[&package.id].name.as_str() == lib)
            .collect::<Vec<_>>();
        match pyo3_packages.as_slice() {
            [pyo3_crate] => {
                let generate_import_lib = pyo3_crate
                    .features
                    .iter()
                    .any(|x| x == "generate-abi3-import-lib");
                return Ok(generate_import_lib);
            }
            _ => continue,
        }
    }
    Ok(false)
}

/// Tries to determine the [BridgeModel] for the target crate
pub fn find_bridge(cargo_metadata: &Metadata, bridge: Option<&str>) -> Result<BridgeModel> {
    let resolve = cargo_metadata
        .resolve
        .as_ref()
        .ok_or_else(|| format_err!("Expected to get a dependency graph from cargo"))?;

    let deps: HashMap<&str, &Node> = resolve
        .nodes
        .iter()
        .map(|node| (cargo_metadata[&node.id].name.as_ref(), node))
        .collect();
    let packages: HashMap<&str, &cargo_metadata::Package> = cargo_metadata
        .packages
        .iter()
        .filter_map(|pkg| {
            let name = &pkg.name;
            if name == "pyo3" || name == "pyo3-ffi" || name == "cpython" {
                Some((name.as_ref(), pkg))
            } else {
                None
            }
        })
        .collect();

    let bridge = if let Some(bindings) = bridge {
        if bindings == "cffi" {
            BridgeModel::Cffi
        } else if bindings == "bin" {
            println!("🔗 Found bin bindings");
            BridgeModel::Bin
        } else {
            if !deps.contains_key(bindings) {
                bail!(
                    "The bindings crate {} was not found in the dependencies list",
                    bindings
                );
            }

            BridgeModel::Bindings(bindings.to_string(), MINIMUM_PYTHON_MINOR)
        }
    } else if deps.get("pyo3").is_some() {
        let ver = &packages["pyo3"].version;
        let minor =
            pyo3_minimum_python_minor_version(ver.major, ver.minor).unwrap_or(MINIMUM_PYTHON_MINOR);
        BridgeModel::Bindings("pyo3".to_string(), minor)
    } else if deps.get("pyo3-ffi").is_some() {
        let ver = &packages["pyo3-ffi"].version;
        let minor = pyo3_ffi_minimum_python_minor_version(ver.major, ver.minor)
            .unwrap_or(MINIMUM_PYTHON_MINOR);
        BridgeModel::Bindings("pyo3-ffi".to_string(), minor)
    } else if deps.contains_key("cpython") {
        println!("🔗 Found rust-cpython bindings");
        BridgeModel::Bindings("rust_cpython".to_string(), MINIMUM_PYTHON_MINOR)
    } else {
        let package = cargo_metadata
            .root_package()
            .context("Expected cargo to return metadata with root_package")?;
        let targets: Vec<_> = package
            .targets
            .iter()
            .flat_map(|target| target.crate_types.iter())
            .map(String::as_str)
            .collect();

        if targets.contains(&"cdylib") {
            BridgeModel::Cffi
        } else if targets.contains(&"bin") {
            BridgeModel::Bin
        } else {
            bail!("Couldn't detect the binding type; Please specify them with --bindings/-b")
        }
    };

    for &lib in PYO3_BINDING_CRATES.iter() {
        if bridge.is_bindings(lib) {
            let pyo3_node = deps[lib];
            if !pyo3_node.features.contains(&"extension-module".to_string()) {
                let version = cargo_metadata[&pyo3_node.id].version.to_string();
                println!(
                    "⚠️  Warning: You're building a library without activating {}'s \
                     `extension-module` feature. \
                     See https://pyo3.rs/v{}/building_and_distribution.html#linking",
                    lib, version
                );
            }

            return if let Some((major, minor)) = has_abi3(cargo_metadata)? {
                println!(
                    "🔗 Found {} bindings with abi3 support for Python ≥ {}.{}",
                    lib, major, minor
                );
                Ok(BridgeModel::BindingsAbi3(major, minor))
            } else {
                println!("🔗 Found {} bindings", lib);
                Ok(bridge)
            };
        }
    }

    Ok(bridge)
}

/// Shared between cffi and pyo3-abi3
fn find_single_python_interpreter(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    bridge_name: &str,
) -> Result<PythonInterpreter> {
    let err_message = "Failed to find a python interpreter";

    let executable = if interpreter.is_empty() {
        target.get_python()
    } else if interpreter.len() == 1 {
        interpreter[0].clone()
    } else {
        bail!(
            "You can only specify one python interpreter for {}",
            bridge_name
        );
    };

    let interpreter = PythonInterpreter::check_executable(executable, target, bridge)
        .context(format_err!(err_message))?
        .ok_or_else(|| format_err!(err_message))?;
    Ok(interpreter)
}

/// Finds the appropriate amount for python versions for each [BridgeModel].
///
/// This means all for bindings, one for cffi and zero for bin.
pub fn find_interpreter(
    bridge: &BridgeModel,
    interpreter: &[PathBuf],
    target: &Target,
    min_python_minor: Option<usize>,
    generate_abi3_import_lib: bool,
) -> Result<Vec<PythonInterpreter>> {
    match bridge {
        BridgeModel::Bindings(binding_name, _) => {
            let mut interpreter = if !interpreter.is_empty() {
                PythonInterpreter::check_executables(interpreter, target, bridge)
                    .context("The given list of python interpreters is invalid")?
            } else {
                PythonInterpreter::find_all(target, bridge, min_python_minor)
                    .context("Finding python interpreters failed")?
            };

            if interpreter.is_empty() {
                if let Some(minor) = min_python_minor {
                    bail!("Couldn't find any python interpreters with version >= 3.{}. Please specify at least one with -i", minor);
                } else {
                    bail!("Couldn't find any python interpreters. Please specify at least one with -i");
                }
            }

            if binding_name.starts_with("pyo3") && target.is_unix() && target.cross_compiling() {
                if let Some(cross_lib_dir) = std::env::var_os("PYO3_CROSS_LIB_DIR") {
                    println!("⚠️ Cross-compiling is poorly supported");
                    let host_python = &interpreter[0];
                    println!(
                        "🐍 Using host {} for cross-compiling preparation",
                        host_python
                    );
                    // pyo3
                    env::set_var("PYO3_PYTHON", &host_python.executable);
                    // rust-cpython, and legacy pyo3 versions
                    env::set_var("PYTHON_SYS_EXECUTABLE", &host_python.executable);

                    let sysconfig_path = find_sysconfigdata(cross_lib_dir.as_ref(), target)?;
                    env::set_var(
                        "MATURIN_PYTHON_SYSCONFIGDATA_DIR",
                        sysconfig_path.parent().unwrap(),
                    );

                    let sysconfig_data = parse_sysconfigdata(host_python, sysconfig_path)?;
                    let major = sysconfig_data
                        .get("version_major")
                        .context("version_major is not defined")?
                        .parse::<usize>()
                        .context("Could not parse value of version_major")?;
                    let minor = sysconfig_data
                        .get("version_minor")
                        .context("version_minor is not defined")?
                        .parse::<usize>()
                        .context("Could not parse value of version_minor")?;
                    let abiflags = sysconfig_data
                        .get("ABIFLAGS")
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    let ext_suffix = sysconfig_data
                        .get("EXT_SUFFIX")
                        .context("syconfig didn't define an `EXT_SUFFIX` ಠ_ಠ")?;
                    let abi_tag = sysconfig_data
                        .get("SOABI")
                        .and_then(|abi| abi.split('-').nth(1).map(ToString::to_string));
                    let interpreter_kind = sysconfig_data
                        .get("SOABI")
                        .and_then(|tag| {
                            if tag.starts_with("pypy") {
                                Some(InterpreterKind::PyPy)
                            } else if tag.starts_with("cpython") {
                                Some(InterpreterKind::CPython)
                            } else {
                                None
                            }
                        })
                        .context("unsupported Python interpreter")?;
                    interpreter = vec![PythonInterpreter {
                        major,
                        minor,
                        abiflags,
                        target: target.clone(),
                        executable: PathBuf::new(),
                        ext_suffix: ext_suffix.to_string(),
                        interpreter_kind,
                        abi_tag,
                        libs_dir: PathBuf::from(cross_lib_dir),
                        platform: None,
                        runnable: false,
                    }];
                }
            }

            println!(
                "🐍 Found {}",
                interpreter
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<String>>()
                    .join(", ")
            );

            Ok(interpreter)
        }
        BridgeModel::Cffi => {
            let interpreter = find_single_python_interpreter(bridge, interpreter, target, "cffi")?;
            println!("🐍 Using {} to generate the cffi bindings", interpreter);
            Ok(vec![interpreter])
        }
        BridgeModel::Bin => Ok(vec![]),
        BridgeModel::BindingsAbi3(major, minor) => {
            let interpreter = if !interpreter.is_empty() {
                PythonInterpreter::check_executables(interpreter, target, bridge)
                    .unwrap_or_default()
            } else {
                PythonInterpreter::find_all(target, bridge, Some(*minor as usize))
                    .unwrap_or_default()
            };
            // Ideally, we wouldn't want to use any python interpreter without abi3 at all.
            // Unfortunately, on windows we need one to figure out base_prefix for a linker
            // argument.
            if target.is_windows() {
                if let Some(manual_base_prefix) = std::env::var_os("PYO3_CROSS_LIB_DIR") {
                    // PYO3_CROSS_LIB_DIR should point to the `libs` directory inside base_prefix
                    // when cross compiling, so we fake a python interpreter matching it
                    println!("⚠️  Cross-compiling is poorly supported");
                    Ok(vec![PythonInterpreter {
                        major: *major as usize,
                        minor: *minor as usize,
                        abiflags: "".to_string(),
                        target: target.clone(),
                        executable: PathBuf::new(),
                        ext_suffix: ".pyd".to_string(),
                        interpreter_kind: InterpreterKind::CPython,
                        abi_tag: None,
                        libs_dir: PathBuf::from(manual_base_prefix),
                        platform: None,
                        runnable: false,
                    }])
                } else if let Some(interp) = interpreter.get(0) {
                    println!("🐍 Using {} to generate to link bindings (With abi3, an interpreter is only required on windows)", interp);
                    Ok(interpreter)
                } else if generate_abi3_import_lib {
                    println!("🐍 Not using a specific python interpreter (Automatically generating windows import library)");
                    // fake a python interpreter
                    Ok(vec![PythonInterpreter {
                        major: *major as usize,
                        minor: *minor as usize,
                        abiflags: "".to_string(),
                        target: target.clone(),
                        executable: PathBuf::new(),
                        ext_suffix: ".pyd".to_string(),
                        interpreter_kind: InterpreterKind::CPython,
                        abi_tag: None,
                        libs_dir: PathBuf::new(),
                        platform: None,
                        runnable: false,
                    }])
                } else {
                    bail!("Failed to find a python interpreter");
                }
            } else {
                println!("🐍 Not using a specific python interpreter");
                Ok(interpreter)
            }
        }
    }
}

/// Helper function that calls shlex on all extra args given
fn split_extra_args(given_args: &[String]) -> Result<Vec<String>> {
    let mut splitted_args = vec![];
    for arg in given_args {
        match shlex::split(arg) {
            Some(split) => splitted_args.extend(split),
            None => {
                bail!(
                    "Couldn't split argument from `--cargo-extra-args`: '{}'",
                    arg
                );
            }
        }
    }
    Ok(splitted_args)
}

/// We need to pass the global flags to cargo metadata
/// (https://github.com/PyO3/maturin/issues/211 and https://github.com/PyO3/maturin/issues/472),
/// but we can't pass all the extra args, as e.g. `--target` isn't supported, so this tries to
/// extract the arguments for cargo metadata
///
/// There are flags (without value) and options (with value). The options value be passed
/// in the same string as its name or in the next one. For this naive parsing logic, we
/// assume that the value is in the next argument if the argument string equals the name,
/// otherwise it's in the same argument and the next argument is unrelated.
fn extract_cargo_metadata_args(cargo_extra_args: &[String]) -> Result<Vec<String>> {
    // flags name and whether it has a value
    let known_prefixes = vec![
        ("--frozen", false),
        ("--locked", false),
        ("--offline", false),
        ("-Z", true),
        ("--features", true),
        ("--all-features", false),
        ("--no-default-features", false),
    ];
    let mut cargo_metadata_extra_args = vec![];
    let mut args_iter = cargo_extra_args.iter();
    // We do manual iteration so we can take and skip the value of an option that is in the next
    // argument
    while let Some(arg) = args_iter.next() {
        // Does it match any of the cargo metadata arguments?
        if let Some((prefix, has_arg)) = known_prefixes
            .iter()
            .find(|(prefix, _)| arg.starts_with(prefix))
        {
            cargo_metadata_extra_args.push(arg.to_string());
            // Do we also need to take the next argument?
            if arg == prefix && *has_arg {
                let value = args_iter.next().context(format!(
                    "Can't parse cargo-extra-args: {} is expected to have an argument",
                    prefix
                ))?;
                cargo_metadata_extra_args.push(value.to_owned());
            }
        }
    }
    Ok(cargo_metadata_extra_args)
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use super::*;

    #[test]
    fn test_find_bridge_pyo3() {
        let pyo3_mixed = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-mixed").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_mixed, None),
            Ok(BridgeModel::Bindings(..))
        ));
        assert!(matches!(
            find_bridge(&pyo3_mixed, Some("pyo3")),
            Ok(BridgeModel::Bindings(..))
        ));

        assert!(find_bridge(&pyo3_mixed, Some("rust-cpython")).is_err());
    }

    #[test]
    fn test_find_bridge_pyo3_abi3() {
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_pure, None),
            Ok(BridgeModel::BindingsAbi3(3, 7))
        ));
        assert!(matches!(
            find_bridge(&pyo3_pure, Some("pyo3")),
            Ok(BridgeModel::BindingsAbi3(3, 7))
        ));
        assert!(find_bridge(&pyo3_pure, Some("rust-cpython")).is_err());
    }

    #[test]
    fn test_find_bridge_pyo3_feature() {
        let pyo3_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-feature").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert!(find_bridge(&pyo3_pure, None).is_err());

        let pyo3_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/pyo3-feature").join("Cargo.toml"))
            .other_options(vec!["--features=pyo3".to_string()])
            .exec()
            .unwrap();

        assert!(matches!(
            find_bridge(&pyo3_pure, None).unwrap(),
            BridgeModel::Bindings(..)
        ));
    }

    #[test]
    fn test_find_bridge_cffi() {
        let cffi_pure = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/cffi-pure").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert_eq!(
            find_bridge(&cffi_pure, Some("cffi")).unwrap(),
            BridgeModel::Cffi
        );
        assert_eq!(find_bridge(&cffi_pure, None).unwrap(), BridgeModel::Cffi);

        assert!(find_bridge(&cffi_pure, Some("rust-cpython")).is_err());
        assert!(find_bridge(&cffi_pure, Some("pyo3")).is_err());
    }

    #[test]
    fn test_find_bridge_bin() {
        let hello_world = MetadataCommand::new()
            .manifest_path(&Path::new("test-crates/hello-world").join("Cargo.toml"))
            .exec()
            .unwrap();

        assert_eq!(
            find_bridge(&hello_world, Some("bin")).unwrap(),
            BridgeModel::Bin
        );
        assert_eq!(find_bridge(&hello_world, None).unwrap(), BridgeModel::Bin);

        assert!(find_bridge(&hello_world, Some("rust-cpython")).is_err());
        assert!(find_bridge(&hello_world, Some("pyo3")).is_err());
    }

    #[test]
    fn test_argument_splitting() {
        let mut options = BuildOptions::default();
        options.cargo_extra_args.push("--features log".to_string());
        options.bindings = Some("bin".to_string());
        let context = options.into_build_context(false, false, false).unwrap();
        assert_eq!(context.cargo_extra_args, vec!["--features", "log"])
    }

    #[test]
    fn test_old_extra_feature_args() {
        let cargo_extra_args = "--no-default-features --features a --target x86_64-unknown-linux-musl --features=c --lib";
        let cargo_extra_args = split_extra_args(&[cargo_extra_args.to_string()]).unwrap();
        let cargo_metadata_extra_args = extract_cargo_metadata_args(&cargo_extra_args).unwrap();
        assert_eq!(
            cargo_metadata_extra_args,
            vec!["--no-default-features", "--features", "a", "--features=c"]
        );
    }

    #[test]
    fn test_extract_cargo_metadata_args() {
        let args: Vec<_> = vec![
            "--locked",
            "--features=my-feature",
            "--unbeknownst",
            "--features",
            "other-feature",
            "--target",
            "x86_64-unknown-linux-musl",
            "-Zunstable-options",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();

        let expected = vec![
            "--locked",
            "--features=my-feature",
            "--features",
            "other-feature",
            "-Zunstable-options",
        ];

        assert_eq!(extract_cargo_metadata_args(&args).unwrap(), expected);
    }

    #[test]
    fn test_get_min_python_minor() {
        // Nothing specified
        let cargo_toml = CargoToml::from_path("test-crates/pyo3-pure/Cargo.toml").unwrap();
        let metadata21 =
            Metadata21::from_cargo_toml(&cargo_toml, &"test-crates/pyo3-pure").unwrap();
        assert_eq!(get_min_python_minor(&metadata21), None);
    }
}
