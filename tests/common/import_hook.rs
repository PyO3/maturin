use crate::common::{create_virtualenv, test_python_path};
use anyhow::{bail, Result};
use maturin::{BuildOptions, CargoOptions, Target};
use regex::RegexBuilder;
use serde_json;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, str, thread};

pub fn test_import_hook(
    virtualenv_name: &str,
    test_specifier: &str,
    extra_packages: &[&str],
    extra_envs: &[(&str, &str)],
    verbose: bool,
) -> Result<()> {
    let python = test_python_path().map(PathBuf::from).unwrap_or_else(|| {
        let target = Target::from_target_triple(None).unwrap();
        target.get_python()
    });

    let (venv_dir, python) = create_virtualenv(virtualenv_name, Some(python)).unwrap();

    let mut packages_to_install = vec!["pytest", "uniffi-bindgen", "cffi", "filelock"];
    packages_to_install.extend(extra_packages);
    for package_name in packages_to_install {
        if verbose {
            println!("installing {package_name}");
        }
        let status = Command::new(&python)
            .args([
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                package_name,
            ])
            .status()
            .unwrap();
        if !status.success() {
            bail!("failed to install: {package_name}");
        }
    }

    let path = env::var_os("PATH").unwrap();
    let mut paths = env::split_paths(&path).collect::<Vec<_>>();
    paths.insert(0, venv_dir.join("bin"));
    paths.insert(
        0,
        Path::new(env!("CARGO_BIN_EXE_maturin"))
            .parent()
            .unwrap()
            .to_path_buf(),
    );
    let path = env::join_paths(paths).unwrap();

    let output = Command::new(&python)
        .args(["-m", "pytest", test_specifier])
        .env("PATH", path)
        .env("VIRTUAL_ENV", venv_dir)
        .envs(extra_envs.iter().cloned())
        .output()
        .unwrap();

    if !output.status.success() {
        bail!(
            "import hook test failed: {}\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
            output.status,
            str::from_utf8(&output.stdout)?.trim(),
            str::from_utf8(&output.stderr)?.trim(),
        );
    } else if verbose {
        println!(
            "import hook test finished:\n--- Stdout:\n{}\n--- Stderr:\n{}\n---\n",
            str::from_utf8(&output.stdout)?.trim(),
            str::from_utf8(&output.stderr)?.trim(),
        )
    }
    Ok(())
}

pub fn test_import_hook_parallel(
    virtualenv_name: &str,
    module: &Path,
    extra_packages: &[&str],
    extra_envs: &[(&str, &str)],
    verbose: bool,
) -> Result<()> {
    let functions = get_top_level_tests(module).unwrap();

    thread::scope(|s| {
        let mut handles = vec![];
        for function_name in &functions {
            let test_specifier = format!("{}::{}", module.to_str().unwrap(), function_name);
            let virtualenv_name = format!("{virtualenv_name}_{function_name}");
            let mut extra_envs_this_test = extra_envs.to_vec();
            extra_envs_this_test.push(("MATURIN_TEST_NAME", function_name));
            let handle = s.spawn(move || {
                test_import_hook(
                    &virtualenv_name,
                    &test_specifier,
                    extra_packages,
                    &extra_envs_this_test,
                    verbose,
                )
                .unwrap()
            });
            handles.push((function_name, handle));
        }
        for (function_name, handle) in handles {
            handle.join().unwrap();
            println!("test {function_name}: passed")
        }
    });
    Ok(())
}

fn get_top_level_tests(module: &Path) -> Result<Vec<String>> {
    let source = String::from_utf8(fs::read(module)?)?;
    let function_pattern = RegexBuilder::new("^def (test_[^(]+)[(]")
        .multi_line(true)
        .build()?;
    let class_pattern = RegexBuilder::new("^class (Test[^:]+):")
        .multi_line(true)
        .build()?;
    let mut top_level_tests = vec![];
    for pattern in [function_pattern, class_pattern] {
        top_level_tests.extend(
            pattern
                .captures_iter(&source)
                .map(|c| c.get(1).unwrap().as_str().to_owned()),
        )
    }
    Ok(top_level_tests)
}

pub fn resolve_all_packages() -> Result<String> {
    let mut resolved_packages = serde_json::Map::new();
    for path in fs::read_dir("test-crates")? {
        let path = path?.path();
        if path.join("pyproject.toml").exists() {
            let project_name = path.file_name().unwrap().to_str().unwrap().to_owned();
            if project_name == "lib_with_path_dep" {
                // Skip lib_with_path_dep because it's used to test `--locked`
                continue;
            }
            resolved_packages.insert(project_name, resolve_package(&path).unwrap_or(Value::Null));
        }
    }
    Ok(serde_json::to_string(&Value::Object(resolved_packages))?)
}

struct TemporaryChdir {
    old_dir: PathBuf,
}

impl TemporaryChdir {
    pub fn chdir(new_cwd: &Path) -> std::io::Result<Self> {
        let old_dir = env::current_dir()?;
        match env::set_current_dir(new_cwd) {
            Ok(()) => Ok(Self { old_dir }),
            Err(e) => Err(e),
        }
    }
}

impl Drop for TemporaryChdir {
    fn drop(&mut self) {
        env::set_current_dir(&self.old_dir).unwrap();
    }
}

fn resolve_package(project_root: &Path) -> Result<Value> {
    let _cwd = TemporaryChdir::chdir(project_root)?;

    let build_options: BuildOptions = Default::default();
    let build_context = build_options.into_build_context(false, false, false)?;
    let extension_module_dir = if build_context.project_layout.python_module.is_some() {
        Some(build_context.project_layout.rust_module)
    } else {
        None
    };

    Ok(json!({
        "cargo_manifest_path": build_context.manifest_path,
        "python_dir": build_context.project_layout.python_dir,
        "python_module": build_context.project_layout.python_module,
        "module_full_name": build_context.module_name,
        "extension_module_dir": extension_module_dir,
    }))
}

pub fn debug_print_resolved_package(package_path: &Path) {
    let resolved = resolve_package(package_path).unwrap_or(Value::Null);
    println!("{}", serde_json::to_string_pretty(&resolved).unwrap());
}
