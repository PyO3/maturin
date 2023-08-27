use crate::common::{create_virtualenv, test_python_path};
use anyhow::{bail, Result};
use maturin::{BuildOptions, CargoOptions, Target};
use serde_json;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, str};

pub fn test_import_hook(
    virtualenv_name: &str,
    test_script_path: &Path,
    extra_packages: Vec<&str>,
    extra_envs: BTreeMap<&str, &str>,
    verbose: bool,
) -> Result<()> {
    let python = test_python_path().map(PathBuf::from).unwrap_or_else(|| {
        let target = Target::from_target_triple(None).unwrap();
        target.get_python()
    });

    let (venv_dir, python) = create_virtualenv(virtualenv_name, Some(python)).unwrap();

    let pytest_args = vec![
        vec!["pytest"],
        vec!["uniffi-bindgen"],
        vec!["cffi"],
        vec!["-e", "."],
    ];
    let extras: Vec<Vec<&str>> = extra_packages.into_iter().map(|name| vec![name]).collect();
    for args in pytest_args.iter().chain(&extras) {
        if verbose {
            println!("installing {:?}", &args);
        }
        let status = Command::new(&python)
            .args(["-m", "pip", "install", "--disable-pip-version-check"])
            .args(args)
            .status()
            .unwrap();
        if !status.success() {
            bail!("failed to install: {:?}", &args);
        }
    }

    let path = env::var_os("PATH").unwrap();
    let mut paths = env::split_paths(&path).collect::<Vec<_>>();
    paths.insert(0, venv_dir.join("bin"));
    let path = env::join_paths(paths).unwrap();

    let output = Command::new(&python)
        .args(["-m", "pytest", test_script_path.to_str().unwrap()])
        .env("PATH", path)
        .env("VIRTUAL_ENV", venv_dir)
        .envs(extra_envs)
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

pub fn resolve_all_packages() -> Result<String> {
    let mut resolved_packages = serde_json::Map::new();
    for path in fs::read_dir("test-crates")? {
        let path = path?.path();
        if path.join("pyproject.toml").exists() {
            let project_name = path.file_name().unwrap().to_str().unwrap().to_owned();
            resolved_packages.insert(project_name, resolve_package(&path).unwrap_or(Value::Null));
        }
    }
    Ok(serde_json::to_string(&Value::Object(resolved_packages))?)
}

fn resolve_package(project_root: &Path) -> Result<Value> {
    let manifest_path = if project_root.join("Cargo.toml").exists() {
        project_root.join("Cargo.toml")
    } else {
        project_root.join("rust").join("Cargo.toml")
    };

    let build_options = BuildOptions {
        cargo: CargoOptions {
            manifest_path: Some(manifest_path.to_owned()),
            ..Default::default()
        },
        ..Default::default()
    };
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
