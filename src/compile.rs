use crate::build_context::BridgeModel;
use crate::BuildContext;
use crate::PythonInterpreter;
use cargo_metadata;
use failure::{bail, Error, ResultExt};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
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
) -> Result<HashMap<String, PathBuf>, Error> {
    let mut shared_args = vec!["--manifest-path", context.manifest_path.to_str().unwrap()];

    // We need to pass --bins / --lib to set the rustc extra args later
    // TODO: What do we do when there are multiple bin targets?
    match bindings_crate {
        BridgeModel::Bin => shared_args.push("--bins"),
        BridgeModel::Cffi | BridgeModel::Bindings(_) => shared_args.push("--lib"),
    }

    shared_args.extend(context.cargo_extra_args.iter().map(String::as_str));

    if context.release {
        shared_args.push("--release");
    }

    let cargo_args = vec!["rustc", "--message-format", "json"];

    let mut rustc_args: Vec<&str> = context
        .rustc_extra_args
        .iter()
        .map(String::as_str)
        .collect();

    if context.target.is_macos() {
        if let BridgeModel::Bindings(_) = bindings_crate {
            let mac_args = &["-C", "link-arg=-undefined", "-C", "link-arg=dynamic_lookup"];
            rustc_args.extend(mac_args);
        }
    }

    if context.strip {
        rustc_args.extend(&["-C", "link-arg=-s"]);
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

    if let Some(python_interpreter) = python_interpreter {
        build_command.env("PYTHON_SYS_EXECUTABLE", &python_interpreter.executable);
    }

    let mut cargo_build = build_command.spawn().context("Failed to run cargo")?;

    let mut artifacts = HashMap::new();

    let stream = cargo_build
        .stdout
        .take()
        .expect("Cargo build should have a stdout");
    for message in cargo_metadata::parse_messages(stream) {
        match message.context("Failed to parse message coming from cargo")? {
            cargo_metadata::Message::CompilerArtifact(artifact) => {
                let crate_name = &context.cargo_metadata[&artifact.package_id].name;

                // Extract the location of the .so/.dll/etc. from cargo's json output
                if crate_name == &context.metadata21.name {
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
pub fn warn_missing_py_init(artifact: &PathBuf, module_name: &str) -> Result<(), Error> {
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
