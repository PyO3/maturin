use atty;
use atty::Stream;
use failure::{Context, Error, ResultExt};
use indicatif::ProgressBar;
use serde_json;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str;
use BuildContext;
use PythonInterpreter;

/// The (abbreviated) format of `cargo build --build-plan`
/// For the real thing, see
/// https://github.com/rust-lang/cargo/blob/master/src/cargo/core/compiler/build_plan.rs
#[derive(Deserialize)]
struct SerializedBuildPlan {
    invocations: Vec<serde_json::Value>,
    #[allow(dead_code)]
    inputs: Vec<PathBuf>,
}

/// This kind of message is printed by `cargo build --message-format=json
/// --quiet` for a build script
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
///     reason: "build-script-executed",
/// }
/// ```
#[derive(Serialize, Deserialize)]
struct CargoBuildOutput {
    cfgs: Vec<String>,
    env: Vec<String>,
    linked_libs: Vec<String>,
    linked_paths: Vec<String>,
    package_id: String,
    reason: String,
}

/// This kind of message is printed by `cargo build --message-format=json
/// --quiet` for an artifact such as an .so/.dll
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

#[derive(Serialize, Deserialize)]
struct CompilerErrorMessage {
    message: CompilerErrorMessageMessage,
    reason: String,
}

#[derive(Serialize, Deserialize)]
struct CompilerErrorMessageMessage {
    rendered: String,
}

/// Queries the number of tasks through the build plan. This only works on
/// nightly, but that isn't a problem, since pyo3 also only works on nightly
fn get_tasks(shared_args: &[&str]) -> Result<usize, Error> {
    let build_plan = Command::new("cargo")
        // Eventually we want to get rid of the nightly, but for now it's required because
        // the rust-toolchain file is ignored
        .args(&["+nightly", "build", "-Z", "unstable-options", "--build-plan"])
        .args(shared_args)
        .stderr(Stdio::inherit()) // Forward any error to the user
        .output()
        .context("Failed to run cargo")?;

    if !build_plan.status.success() {
        bail!("Failed to get a build plan from cargo");
    }

    let plan: SerializedBuildPlan = serde_json::from_slice(&build_plan.stdout)
        .context("The build plan has an invalid format")?;
    let tasks = plan.invocations.len();
    Ok(tasks)
}

/// Builds the rust crate into a native module (i.e. an .so or .dll) for a
/// specific python version
///
/// Shows a progress bar on a tty
pub fn compile(
    context: &BuildContext,
    python_interpreter: Option<&PythonInterpreter>,
    bindings_crate: Option<String>,
) -> Result<PathBuf, Error> {
    // Some stringly typing to satisfy the borrow checker
    let python_feature = match python_interpreter {
        Some(python_interpreter) => format!(
            "{}/python{}",
            bindings_crate.unwrap(),
            python_interpreter.major
        ),
        None => "".to_string(),
    };

    let mut shared_args = vec![
        // The lib is also built without that flag, but then the json doesn't contain the
        // message we need
        "--lib",
        "--manifest-path",
        context.manifest_path.to_str().unwrap(),
    ];

    if python_feature != "" {
        // This is a workaround for a bug in pyo3's build.rs
        shared_args.extend(&["--features", &python_feature]);
    }

    shared_args.extend(context.cargo_extra_args.iter().map(|x| x.as_str()));

    if atty::is(Stream::Stderr) {
        // Makes cargo only print to stderr on error
        shared_args.push("--quiet");
    }

    if !context.debug {
        shared_args.push("--release");
    }

    let tasks = get_tasks(&shared_args)?;

    let mut rustc_args = context.rustc_extra_args.clone();
    if context.target.is_macos() {
        rustc_args.extend(
            ["-C", "link-arg=-undefined", "-C", "link-arg=dynamic_lookup"]
                .iter()
                .map(ToString::to_string),
        );
    }
    let mut let_binding = Command::new("cargo");
    let build_command = let_binding
        .args(&["+nightly", "rustc", "--message-format", "json"])
        .args(&shared_args)
        .arg("--")
        .args(rustc_args)
        .stdout(Stdio::piped()) // We need to capture the json messages
        .stderr(Stdio::inherit()); // We want to show error messages

    if let Some(python_interpreter) = python_interpreter {
        build_command.env("PYTHON_SYS_EXECUTABLE", &python_interpreter.executable);
    }

    let mut cargo_build = build_command.spawn().context("Failed to run cargo")?;

    let progress_bar = if atty::is(Stream::Stderr) {
        Some(ProgressBar::new(tasks as u64))
    } else {
        None
    };

    let mut artifact = None;
    let reader = BufReader::new(cargo_build.stdout.take().unwrap());
    for line in reader.lines().map(|line| line.unwrap()) {
        if let Some(ref progress_bar) = progress_bar {
            progress_bar.inc(1);
        }

        // Extract the location of the .so/.dll/etc. from cargo's json output
        if let Ok(message) = serde_json::from_str::<CompilerArtifactMessage>(&line) {
            if message.target.name == context.module_name {
                artifact = Some(message);
            }
        }

        // Forward error messages
        if let Ok(message) = serde_json::from_str::<CompilerErrorMessage>(&line) {
            if message.reason == "compiler-message" {
                eprintln!("{}", message.message.rendered);
            }
        }
    }

    if let Some(ref progress_bar) = progress_bar {
        progress_bar.finish_and_clear();
    }

    let status = cargo_build
        .wait()
        .expect("Failed to wait on cargo child process");

    if !status.success() {
        bail!("Cargo build finished with an error")
    }

    let artifact = artifact
        .ok_or_else(|| Context::new("cargo build didn't return information on the cdylib"))?;
    let position = artifact
        .target
        .crate_types
        .iter()
        .position(|target| *target == "cdylib")
        .ok_or_else(|| {
            Context::new(
                "Cargo didn't build a cdylib (Did you miss crate-type = [\"cdylib\"] \
                 in the lib section of your Cargo.toml?)",
            )
        })?;
    let artifact = artifact.filenames[position].clone();

    Ok(artifact)
}
