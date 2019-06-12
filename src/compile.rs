use crate::build_context::BridgeModel;
use crate::BuildContext;
use crate::PythonInterpreter;
use atty;
use atty::Stream;
use cargo_metadata;
use cargo_metadata::Message;
use failure::{bail, format_err, Error, ResultExt};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use serde_json;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str;

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
        .context(format_err!(
            "Failed to get a build plan from cargo with `{}`",
            command_formatted
        ))?;

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
                let packages: Vec<String> = build_plan
                    .invocations
                    .iter()
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
                    eprintln!("⚠ Warning: The build plan is empty ಠ_ಠ");
                }

                Some((progress_bar, packages))
            }
            Err(_) => None,
        }
    } else {
        None
    }
}

fn update_progress(progress_plan: &mut Option<(ProgressBar, Vec<String>)>, crate_name: &str) {
    // The progress bar isn't an exact science and stuff might get out-of-sync,
    // but that isn't big problem since the bar is only to give the user an estimate
    if let Some((ref progress_bar, ref mut packages)) = progress_plan {
        match packages.iter().position(|x| x == crate_name) {
            Some(pos) => {
                packages.remove(pos);
                progress_bar.inc(1);
            }
            None => eprintln!(
                "⚠ Warning: {} was not found in build plan ಠ_ಠ",
                crate_name
            ),
        }

        if let Some(package) = packages.first() {
            progress_bar.set_message(&package);
        }
    }
}

/// Builds the rust crate into a native module (i.e. an .so or .dll) for a
/// specific python version. Returns a mapping from crate type (e.g. cdylib)
/// to artifact location.
///
/// Shows a progress bar on a tty
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
        match message.unwrap() {
            Message::CompilerArtifact(artifact) => {
                let crate_name = &context.cargo_metadata[&artifact.package_id].name;
                update_progress(&mut progress_plan, crate_name);

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
            Message::BuildScriptExecuted(script) => {
                let crate_name = &context.cargo_metadata[&script.package_id].name;
                update_progress(&mut progress_plan, &crate_name);
            }
            Message::CompilerMessage(msg) => {
                if let Some((ref progress_bar, _)) = progress_plan {
                    progress_bar.println(msg.message.to_string());
                } else {
                    eprintln!("{}", msg.message);
                }
            }
            _ => (),
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
