use crate::build_context::BridgeModel;
use crate::BuildContext;
use crate::PythonInterpreter;
use anyhow::{anyhow, bail, Context, Result};
use fat_macho::FatWriter;
use fs_err::{self as fs, File};
use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str;

/// Builds the rust crate into a native module (i.e. an .so or .dll) for a
/// specific python version. Returns a mapping from crate type (e.g. cdylib)
/// to artifact location.
pub fn compile(
    context: &BuildContext,
    python_interpreter: Option<&PythonInterpreter>,
    bindings_crate: &BridgeModel,
) -> Result<HashMap<String, PathBuf>> {
    if context.target.is_macos() && context.universal2 {
        let build_type = match bindings_crate {
            BridgeModel::Bin => "bin",
            _ => "cdylib",
        };
        let aarch64_artifact = compile_target(
            context,
            python_interpreter,
            bindings_crate,
            Some("aarch64-apple-darwin"),
        )
        .context("Failed to build a aarch64 library through cargo")?
        .get(build_type)
        .cloned()
        .ok_or_else(|| {
            if build_type == "cdylib" {
                anyhow!(
                    "Cargo didn't build an aarch64 cdylib. Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?",
                )
            } else {
                anyhow!("Cargo didn't build an aarch64 bin.")
            }
        })?;
        let x86_64_artifact = compile_target(
            context,
            python_interpreter,
            bindings_crate,
            Some("x86_64-apple-darwin"),
        )
        .context("Failed to build a x86_64 library through cargo")?
        .get(build_type)
        .cloned()
        .ok_or_else(|| {
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
            .display()
            .to_string()
            .replace("aarch64-apple-darwin/", "");
        let mut writer = FatWriter::new();
        let aarch64_file = fs::read(aarch64_artifact)?;
        let x86_64_file = fs::read(x86_64_artifact)?;
        writer
            .add(aarch64_file)
            .map_err(|e| anyhow!("Failed to add aarch64 cdylib: {:?}", e))?;
        writer
            .add(x86_64_file)
            .map_err(|e| anyhow!("Failed to add x86_64 cdylib: {:?}", e))?;
        writer
            .write_to_file(&output_path)
            .map_err(|e| anyhow!("Failed to create unversal cdylib: {:?}", e))?;

        let mut result = HashMap::new();
        result.insert(build_type.to_string(), PathBuf::from(output_path));
        Ok(result)
    } else {
        compile_target(context, python_interpreter, bindings_crate, None)
    }
}

fn compile_target(
    context: &BuildContext,
    python_interpreter: Option<&PythonInterpreter>,
    bindings_crate: &BridgeModel,
    target: Option<&str>,
) -> Result<HashMap<String, PathBuf>> {
    let mut shared_args = vec!["--manifest-path", context.manifest_path.to_str().unwrap()];

    // We need to pass --bins / --lib to set the rustc extra args later
    // TODO: What do we do when there are multiple bin targets?
    match bindings_crate {
        BridgeModel::Bin => shared_args.push("--bins"),
        BridgeModel::Cffi | BridgeModel::Bindings(_) | BridgeModel::BindingsAbi3(_, _) => {
            shared_args.push("--lib")
        }
    }

    shared_args.extend(context.cargo_extra_args.iter().map(String::as_str));

    if context.release {
        shared_args.push("--release");
    }
    if let Some(target) = target {
        shared_args.push("--target");
        shared_args.push(target);
    }

    let cargo_args = vec!["rustc", "--message-format", "json"];

    let mut rustc_args: Vec<&str> = context
        .rustc_extra_args
        .iter()
        .map(String::as_str)
        .collect();

    // https://github.com/PyO3/pyo3/issues/88#issuecomment-337744403
    if context.target.is_macos() {
        if let BridgeModel::Bindings(_) | BridgeModel::BindingsAbi3(_, _) = bindings_crate {
            let mac_args = &["-C", "link-arg=-undefined", "-C", "link-arg=dynamic_lookup"];
            rustc_args.extend(mac_args);
        }
    }

    if context.strip {
        rustc_args.extend(&["-C", "link-arg=-s"]);
    }

    let pythonxy_lib_folder;
    if let BridgeModel::BindingsAbi3(_, _) = bindings_crate {
        // NB: We set PYO3_NO_PYTHON further below.
        // On linux, we can build a shared library without the python
        // providing these symbols being present, on mac we can do it with
        // the `-undefined dynamic_lookup` we use above anyway. On windows
        // however, we get an exit code 0xc0000005 if we try the same with
        // `/FORCE:UNDEFINED`, so we still look up the python interpreter
        // and pass the location of the lib with the definitions.
        if context.target.is_windows() {
            let python_interpreter = python_interpreter
                .expect("Must have a python interpreter for building abi3 on windows");
            pythonxy_lib_folder = format!("native={}", python_interpreter.libs_dir.display());
            rustc_args.extend(&["-L", &pythonxy_lib_folder]);
        }
    }

    let build_args: Vec<_> = cargo_args
        .iter()
        .chain(&shared_args)
        .chain(&["--"])
        .chain(&rustc_args)
        .collect();
    let command_str = build_args
        .iter()
        .fold("cargo".to_string(), |acc, x| acc + " " + x);

    let mut let_binding = Command::new("cargo");
    let build_command = let_binding
        .args(&build_args)
        // We need to capture the json messages
        .stdout(Stdio::piped())
        // We can't get colored human and json messages from rustc as they are mutually exclusive,
        // but forwarding stderr is still useful in case there some non-json error
        .stderr(Stdio::inherit());

    if let BridgeModel::BindingsAbi3(_, _) = bindings_crate {
        // This will make pyo3's build script only set some predefined linker
        // arguments without trying to read any python configuration
        build_command.env("PYO3_NO_PYTHON", "1");
    }

    if let Some(python_interpreter) = python_interpreter {
        if bindings_crate.is_bindings("pyo3") {
            build_command.env("PYO3_PYTHON", &python_interpreter.executable);
        }

        // rust-cpython, and legacy pyo3 versions
        build_command.env("PYTHON_SYS_EXECUTABLE", &python_interpreter.executable);
    }

    let mut cargo_build = build_command.spawn().context("Failed to run cargo")?;

    let mut artifacts = HashMap::new();

    let stream = cargo_build
        .stdout
        .take()
        .expect("Cargo build should have a stdout");
    for message in cargo_metadata::Message::parse_stream(BufReader::new(stream)) {
        match message.context("Failed to parse message coming from cargo")? {
            cargo_metadata::Message::CompilerArtifact(artifact) => {
                let package_in_metadata = context
                    .cargo_metadata
                    .packages
                    .iter()
                    .find(|package| package.id == artifact.package_id);
                let crate_name = match package_in_metadata {
                    Some(package) => &package.name,
                    None => {
                        // This is a spurious error I don't really understand
                        println!(
                            "⚠  Warning: The package {} wasn't listed in `cargo metadata`",
                            artifact.package_id
                        );
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
                        artifacts.insert(crate_type, filename);
                    }
                }
            }
            cargo_metadata::Message::CompilerMessage(msg) => {
                println!("{}", msg.message);
            }
            _ => (),
        }
    }

    let status = cargo_build
        .wait()
        .expect("Failed to wait on cargo child process");

    if !status.success() {
        bail!(
            r#"Cargo build finished with "{}": `{}`"#,
            status,
            command_str
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
/// Currently the check is only run on linux
pub fn warn_missing_py_init(artifact: &PathBuf, module_name: &str) -> Result<()> {
    let py_init = format!("PyInit_{}", module_name);
    let mut fd = File::open(&artifact)?;
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
        _ => {
            // Currently, only linux is implemented
            found = true
        }
    }

    if !found {
        println!(
            "⚠  Warning: Couldn't find the symbol `{}` in the native library. \
             Python will fail to import this module. \
             If you're using pyo3, check that `#[pymodule]` uses `{}` as module name",
            py_init, module_name
        )
    }

    Ok(())
}
