use failure::{Context, Error, ResultExt};
use serde_json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str;
use BuildContext;
use PythonInterpreter;

/// This kind of message is printed by `cargo build --message-format=json --quiet` for a build
/// script
///
/// Example with python3.6 on ubuntu 18.04.1:
///
/// ```text
/// CargoBuildConfig {
///     cfgs: ["Py_3_5", "Py_3_6", "Py_3", "py_sys_config=\"WITH_THREAD\""],
///     env: [],
///     linked_libs: ["python3.6m"],
///     linked_paths: ["native=/usr/lib"],
///     package_id: "pyo3 0.2.5 (path+file:///home/konsti/capybara/pyo3)",
///     reason: "build-script-executed"
/// }
/// ```
#[derive(Serialize, Deserialize)]
struct CargoBuildOutput {
    pub cfgs: Vec<String>,
    pub env: Vec<String>,
    pub linked_libs: Vec<String>,
    pub linked_paths: Vec<String>,
    pub package_id: String,
    pub reason: String,
}

/// This kind of message is printed by `cargo build --message-format=json --quiet` for an artifact
/// such as an .so/.dll
#[derive(Serialize, Deserialize)]
struct CompilerArtifactMessage {
    filenames: Vec<PathBuf>,
    target: CompilerTargetMessage,
}

#[derive(Serialize, Deserialize)]
struct CompilerTargetMessage {
    crate_types: Vec<String>,
    name: String,
}

/// Builds the rust crate into a native module (i.e. an .so or .dll) for a specific python version
pub fn build_rust(
    lib_name: &str,
    manifest_file: &Path,
    context: &BuildContext,
    python_interpreter: &PythonInterpreter,
) -> Result<PathBuf, Error> {
    println!("Building the crate for {}", python_interpreter);
    let python_version_feature = format!(
        "{}/python{}",
        context.binding_crate, python_interpreter.major
    );

    let mut args = vec![
        "build",
        // The lib is also built without that flag, but then the json doesn't contain the
        // message we need
        "--lib",
        "--message-format=json",
        "--manifest-path",
        manifest_file.to_str().unwrap(),
        // This is a workaround for a bug in pyo3's build.rs
        "--features",
        &python_version_feature,
    ];

    if !context.debug {
        args.push("--release");
    }

    let build_messages = Command::new("cargo")
        .args(&args)
        .env("PYTHON_SYS_EXECUTABLE", &python_interpreter.executable)
        .stderr(Stdio::inherit()) // Forwards cargo's messages
        .output()
        .context("Failed to run cargo")?;

    if !build_messages.status.success() {
        bail!("Cargo failed to run")
    }

    // It's json and it's even coming from rust code, so it must be utf8
    let binding_lib_output = str::from_utf8(&build_messages.stdout).unwrap();
    let binding_lib: Option<CargoBuildOutput> = binding_lib_output
        .lines()
        .filter_map(|line| serde_json::from_str::<CargoBuildOutput>(line).ok())
        .find(|config| config.package_id.starts_with(&context.binding_crate));

    let binding_lib = binding_lib.and_then(|binding_lib| {
        if binding_lib.linked_libs.len() == 1 {
            Some(binding_lib.linked_libs[0].clone())
        } else {
            None
        }
    });

    if let Some(_version_line) = binding_lib {
        // TODO: Validate that the python interpreteer used by pyo3 is the expected one
        // This is blocked on https://github.com/rust-lang/cargo/issues/5602 being released to stable
    };

    // Extract the location of the .so/.dll/etc. from cargo's json output
    let message = binding_lib_output
        .lines()
        .filter_map(|line| serde_json::from_str::<CompilerArtifactMessage>(line).ok())
        .find(|artifact| artifact.target.name == lib_name)
        .ok_or_else(|| Context::new("cargo build didn't return the expected information"))?;

    let position = message
        .target
        .crate_types
        .iter()
        .position(|target| *target == "cdylib")
        .ok_or_else(|| {
            Context::new(r#"Cargo didn't build a cdylib (Did you miss crate-type = ["cdylib"] in the lib section of your Cargo.toml?)"#)
        })?;
    let artifact = message.filenames[position].clone();

    Ok(artifact)
}
