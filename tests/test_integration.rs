use crate::common::check_installed;
use crate::common::install_cffi;
use pyo3_pack::{BuildOptions, Target};
use std::path::Path;
use std::process::{Command, Stdio};
use std::str;
use structopt::StructOpt;

mod common;

// Y U NO accept windows path prefix, pip?
// Anyways, here's shepmasters stack overflow solution
// https://stackoverflow.com/a/50323079/3549270
#[cfg(not(target_os = "windows"))]
fn adjust_canonicalization(p: impl AsRef<Path>) -> String {
    p.as_ref().display().to_string()
}

#[cfg(target_os = "windows")]
fn adjust_canonicalization(p: impl AsRef<Path>) -> String {
    const VERBATIM_PREFIX: &str = r#"\\?\"#;
    let p = p.as_ref().display().to_string();
    if p.starts_with(VERBATIM_PREFIX) {
        p[VERBATIM_PREFIX.len()..].to_string()
    } else {
        p
    }
}

#[test]
#[cfg(not(feature = "appveyor-test"))]
fn test_integration_get_fourtytwo() {
    test_integration(Path::new("get-fourtytwo"), None);
}

#[cfg(target_os = "windows")]
#[test]
fn test_integration_get_fourtytwo_conda() {
    test_integration_conda(Path::new("get-fourtytwo"), None);
}

#[test]
fn test_integration_points() {
    test_integration(Path::new("points"), Some("cffi".to_string()));
}

#[test]
fn test_integration_hello_world() {
    test_integration(Path::new("hello-world"), Some("bin".to_string()));
}

/// For each installed python version, this builds a wheel, creates a virtualenv if it
/// doesn't exist, installs the package and runs check_installed.py
fn test_integration(package: &Path, bindings: Option<String>) {
    let target = Target::from_target_triple(None).unwrap();

    let package_string = package.join("Cargo.toml").display().to_string();

    // The first string is ignored by clap
    let cli = if let Some(ref bindings) = bindings {
        vec![
            "build",
            "--manifest-path",
            &package_string,
            "--bindings",
            bindings,
        ]
    } else {
        vec!["build", "--manifest-path", &package_string]
    };

    let options = BuildOptions::from_iter_safe(cli).unwrap();

    install_cffi(&target.get_python());
    let wheels = options
        .into_build_context(false, false)
        .unwrap()
        .build_wheels()
        .unwrap();

    for (filename, supported_version, python_interpreter) in wheels {
        let venv_dir = if supported_version == "py2.py3" {
            package.join("venv_cffi")
        } else {
            package.join(format!(
                "venv{}.{}",
                supported_version.chars().nth(2usize).unwrap(),
                supported_version.chars().nth(3usize).unwrap()
            ))
        };

        if !venv_dir.is_dir() {
            let venv_py_version = if let Some(ref python_interpreter) = python_interpreter {
                python_interpreter.executable.clone()
            } else {
                target.get_python()
            };

            let output = Command::new("virtualenv")
                .args(&[
                    "-p",
                    &venv_py_version.display().to_string(),
                    &venv_dir.display().to_string(),
                ])
                .stderr(Stdio::inherit())
                .output()
                .unwrap();
            if !output.status.success() {
                panic!();
            }
        }

        let python = target.get_venv_python(&venv_dir);

        let output = Command::new(&python)
            .args(&[
                "-m",
                "pip",
                "install",
                "--force-reinstall",
                &adjust_canonicalization(filename),
            ])
            .stderr(Stdio::inherit())
            .output()
            .unwrap();
        if !output.status.success() {
            panic!();
        }

        let output = Command::new(&python)
            .args(&["-m", "pip", "install", "cffi"])
            .output()
            .unwrap();
        if !output.status.success() {
            panic!(
                "pip failed: {} \n--- Stdout:\n{}\n--- Stderr:\n{}",
                output.status,
                str::from_utf8(&output.stdout).unwrap(),
                str::from_utf8(&output.stderr).unwrap()
            );
        }

        check_installed(&package, &python).unwrap();
    }
}

/// Creates conda environments
#[cfg(target_os = "windows")]
fn create_conda_env(name: &str, major: usize, minor: usize) {
    Command::new("conda")
        .arg("create")
        .arg("-n")
        .arg(name)
        .arg(format!("python={}.{}", major, minor))
        .arg("-q")
        .arg("-y")
        .output()
        .expect("Conda not available.");
}

#[cfg(target_os = "windows")]
fn test_integration_conda(package: &Path, bindings: Option<String>) {
    use std::env;
    use std::path::PathBuf;

    let package_string = package.join("Cargo.toml").display().to_string();

    // Since the python launcher has precedence over conda, we need to deactivate it.
    // We do so by shadowing it with our own hello world binary.
    let original_path = env::var_os("PATH").expect("PATH is not defined");
    let py_dir = env::current_dir()
        .unwrap()
        .join("test-data")
        .to_str()
        .unwrap()
        .to_string();
    let mocked_path = py_dir + ";" + original_path.to_str().unwrap();
    env::set_var("PATH", mocked_path);

    // Create environments to build against, prepended with "A" to ensure that integration
    // tests are executed with these environments
    create_conda_env("A-pyo3-build-env-27", 2, 7);
    create_conda_env("A-pyo3-build-env-35", 3, 5);
    create_conda_env("A-pyo3-build-env-36", 3, 6);
    create_conda_env("A-pyo3-build-env-37", 3, 7);

    // The first string is ignored by clap
    let cli = if let Some(ref bindings) = bindings {
        vec![
            "build",
            "--manifest-path",
            &package_string,
            "--bindings",
            bindings,
        ]
    } else {
        vec!["build", "--manifest-path", &package_string]
    };

    let options = BuildOptions::from_iter_safe(cli).unwrap();

    let wheels = options
        .into_build_context(false, false)
        .unwrap()
        .build_wheels()
        .unwrap();

    let mut conda_wheels: Vec<(PathBuf, PathBuf)> = vec![];
    for (filename, _, python_interpreter) in wheels {
        if let Some(pi) = python_interpreter {
            let executable = pi.executable;
            if executable.to_str().unwrap().contains("pyo3-build-env-") {
                conda_wheels.push((filename, executable))
            }
        }
    }

    assert_eq!(
        4,
        conda_wheels.len(),
        "Error creating or detecting conda environments."
    );
    for (wheel_file, executable) in conda_wheels {
        let output = Command::new(&executable)
            .args(&[
                "-m",
                "pip",
                "install",
                "--force-reinstall",
                &adjust_canonicalization(wheel_file),
            ])
            .stderr(Stdio::inherit())
            .output()
            .unwrap();
        if !output.status.success() {
            panic!();
        }
        check_installed(&package, &executable).unwrap();
    }
}
