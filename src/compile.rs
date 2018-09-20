use atty;
use atty::Stream;
use build_context::BridgeModel;
use failure::{Error, ResultExt};
use indicatif::{ProgressBar, ProgressStyle};
use serde_json;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str;
use BuildContext;
use PythonInterpreter;

#[derive(Deserialize, Debug, Clone)]
struct BuildPlanEntry {
    package_name: String,
    program: String,
}

/// The (abbreviated) format of `cargo build --build-plan`
/// For the real thing, see
/// https://github.com/rust-lang/cargo/blob/master/src/cargo/core/compiler/build_plan.rs
#[derive(Deserialize, Debug, Clone)]
struct SerializedBuildPlan {
    invocations: Vec<BuildPlanEntry>,
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
#[derive(Serialize, Deserialize, Debug, Clone)]
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
#[derive(Serialize, Deserialize, Debug, Clone)]
struct CompilerArtifactMessage {
    filenames: Vec<PathBuf>,
    target: CompilerTargetMessage,
    package_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CompilerTargetMessage {
    crate_types: Vec<String>,
    name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CompilerErrorMessage {
    message: CompilerErrorMessageMessage,
    reason: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CompilerErrorMessageMessage {
    rendered: String,
}

/// Queries the number of tasks through the build plan. This only works on
/// nightly, but that isn't a problem, since pyo3 also only works on nightly
fn get_build_plan(shared_args: &[&str]) -> Result<SerializedBuildPlan, Error> {
    let build_plan_args = &[
        "+nightly",
        "build",
        "-Z",
        "unstable-options",
        "--build-plan",
    ];

    let command_formatted = ["cargo"]
        .iter()
        .chain(build_plan_args)
        .chain(shared_args)
        .map(ToString::to_string)
        .collect::<Vec<String>>()
        .join(" ");

    let build_plan = Command::new("cargo")
        // Eventually we want to get rid of the nightly, but for now it's required because
        // the rust-toolchain file is ignored
        .args(build_plan_args)
        .args(shared_args)
        .output()
        .map_err(|e| {
            format_err!(
                "Failed to get a build plan from cargo: {} ({})",
                e,
                command_formatted
            )
        })?;

    if !build_plan.status.success() {
        bail!(
            "Failed to get a build plan from cargo with '{}': `{}`",
            build_plan.status,
            command_formatted
        );
    }

    let plan: SerializedBuildPlan = serde_json::from_slice(&build_plan.stdout)
        .context("The build plan has an invalid format")?;
    Ok(plan)
}

fn get_progress_plan(shared_args: &[&str]) -> Option<(ProgressBar, Vec<String>)> {
    if atty::is(Stream::Stderr) {
        match get_build_plan(shared_args) {
            Ok(build_plan) => {
                let mut packages: Vec<String> = build_plan
                    .invocations
                    .iter()
                    .filter(|x| x.program == "rustc") // Only those gives artifact messages
                    .map(|x| x.package_name.clone())
                    .collect();

                let progress_bar = ProgressBar::new(packages.len() as u64);
                progress_bar.set_style(
                    ProgressStyle::default_bar()
                        .template("[{bar:60}] {pos:>3}/{len:3} {msg}")
                        .progress_chars("=> "),
                );

                if let Some(first) = packages.first() {
                    progress_bar.set_message(first);
                } else {
                    eprintln!("Warning: The build plan is empty");
                }

                Some((progress_bar, packages))
            }
            Err(_) => None,
        }
    } else {
        None
    }
}

/// Builds the rust crate into a native module (i.e. an .so or .dll) for a
/// specific python version
///
/// Shows a progress bar on a tty
pub fn compile(
    context: &BuildContext,
    python_interpreter: Option<&PythonInterpreter>,
    bindings_crate: &BridgeModel,
) -> Result<HashMap<String, PathBuf>, Error> {
    // Some stringly typing to satisfy the borrow checker
    let python_feature = match python_interpreter {
        Some(python_interpreter) => format!(
            "{}/python{}",
            bindings_crate.unwrap_bindings(),
            python_interpreter.major
        ),
        None => "".to_string(),
    };

    let mut shared_args = vec!["--manifest-path", context.manifest_path.to_str().unwrap()];

    if python_feature != "" {
        // This is a workaround for a bug in pyo3's build.rs
        shared_args.extend(&["--features", &python_feature]);
    }

    // We need to pass --bins / --lib to set the rustc extra args later
    // TODO: What do we do when there are multiple bin targets?
    match bindings_crate {
        BridgeModel::Bin => shared_args.push("--bins"),
        BridgeModel::Cffi | BridgeModel::Bindings(_) => shared_args.push("--lib"),
    }

    shared_args.extend(context.cargo_extra_args.iter().map(|x| x.as_str()));

    if context.release {
        shared_args.push("--release");
    }

    let mut cargo_args = vec!["rustc", "--message-format", "json"];

    // Mimicks cargo's -Z compile-progress, just without the long result log
    let mut progress_plan = get_progress_plan(&shared_args);

    if progress_plan.is_some() {
        // We have out own progess bar, so we don't need cargo's bar
        cargo_args.push("--quiet");
    }

    let mut rustc_args: Vec<&str> = context
        .rustc_extra_args
        .iter()
        .map(|x| x.as_str())
        .collect();

    if context.target.is_macos() {
        if let BridgeModel::Bindings(_) = bindings_crate {
            let mac_args = &["-C", "link-arg=-undefined", "-C", "link-arg=dynamic_lookup"];
            rustc_args.extend(mac_args);
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
        .stdout(Stdio::piped()) // We need to capture the json messages
        .stderr(Stdio::inherit()); // We want to show error messages

    if let Some(python_interpreter) = python_interpreter {
        build_command.env("PYTHON_SYS_EXECUTABLE", &python_interpreter.executable);
    }

    let mut cargo_build = build_command.spawn().context("Failed to run cargo")?;

    let mut artifact_messages = Vec::new();
    let reader = BufReader::new(cargo_build.stdout.take().unwrap());
    for line in reader.lines().map(|line| line.unwrap()) {
        if let Ok(message) = serde_json::from_str::<CompilerArtifactMessage>(&line) {
            // Extract the location of the .so/.dll/etc. from cargo's json output
            let target_name = message.target.name.clone();
            if target_name == context.module_name || target_name == context.metadata21.name {
                artifact_messages.push(message.clone());
            }

            let crate_name = message.package_id.split(" ").nth(0).unwrap().to_string();

            // The progress bar isn't an exact science and stuff might get out-of-sync,
            // but that isn't big problem since the bar is only to give the user an estimate
            if let Some((ref progress_bar, ref mut packages)) = progress_plan {
                match packages.iter().position(|x| x == &crate_name) {
                    Some(pos) => {
                        packages.remove(pos);
                        progress_bar.inc(1);
                    }
                    None => eprintln!("WARN: {} not found in build plan", crate_name),
                }

                if let Some(package) = packages.first() {
                    progress_bar.set_message(&package);
                }
            }
        }

        // Forward error messages
        if let Ok(message) = serde_json::from_str::<CompilerErrorMessage>(&line) {
            if message.reason == "compiler-message" {
                eprintln!("{}", message.message.rendered);
            }
        }
    }

    if let Some((ref progress_bar, _)) = progress_plan {
        progress_bar.finish_and_clear();
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

    let mut artifacts = HashMap::new();
    for message in artifact_messages {
        let tuples = message
            .target
            .crate_types
            .into_iter()
            .zip(message.filenames);
        for (crate_type, filename) in tuples {
            artifacts.insert(crate_type, filename);
        }
    }

    Ok(artifacts)
}
