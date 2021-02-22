//! Caches cargo invocations to make maturin's tests faster
//!
//! For each invocation, a directory inside `target/test-cache` is created with the
//! name `<PYTHON_SYS_EXECUTABLE> <cargo arg1> <cargo arg2> ... <cargo argx>` with some args
//! stripped for path length constrains. It contains a `cargo.stdout`, a `cargo.stderr` and all
//! non-rlib artifacts.

use anyhow::{bail, format_err, Context, Result};
use cargo_metadata::Message;
use std::env;
use std::fs;
use std::fs::File;
use std::io;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::Command;

fn run() -> Result<()> {
    let base_cache_path = Path::new("target").join("test-cache");

    fs::create_dir_all(&base_cache_path).context("Couldn't create cache dir")?;
    let env_args_os_str = env::var_os("PYTHON_SYS_EXECUTABLE").unwrap_or_default();
    let env_args = env_args_os_str
        .into_string()
        .map_err(|e| format_err!("PYTHON_SYS_EXECUTABLE is not valid unicode: {:?}", e))?;
    let cargo_args = env::args().skip(1).collect::<Vec<String>>().join(" ");
    // Assumption: Slash is the only character in the cli args that we must not use for directory names
    let cwd = env::current_dir().unwrap().to_string_lossy().to_string();
    let env_key = env_args.replace(" ", "-").replace("/", "-");
    let cargo_key = cargo_args
        .replace("--message-format json", "")
        .replace("--quiet", "")
        .replace(&cwd, "")
        .replace(" ", "-")
        .replace("/", "-");

    let cache_path = base_cache_path.join(&env_key).join(&cargo_key);
    let stdout_path = cache_path.join("cargo.stdout");
    let stderr_path = cache_path.join("cargo.stderr");

    if stderr_path.is_file() {
        let context_message: &'static str = "Failed to read from capture file";
        // Write the capture stdout and stderr back out
        let mut stdout_file = File::open(stdout_path).context(context_message)?;
        let mut stdout = io::stdout();
        io::copy(&mut stdout_file, &mut stdout).context(context_message)?;

        let mut stderr_file = File::open(stderr_path).context(context_message)?;
        let mut stderr = io::stderr();
        io::copy(&mut stderr_file, &mut stderr).context(context_message)?;
    } else {
        fs::create_dir_all(&cache_path).context(format!(
            "Failed to create cache path {}",
            cache_path.display()
        ))?;
        // Unmock to run the real cargo
        let old_path = env::var_os("PATH").expect("PATH must be set");
        let mut path_split = env::split_paths(&old_path);
        let first_path = path_split.next().expect("PATH must have a first entry");
        if !first_path.join("cargo").is_file() && !first_path.join("cargo.exe").is_file() {
            bail!("The first part of PATH hasn't overwritten cargo");
        }
        let remainder = env::join_paths(path_split).expect("Expected to be able to re-join PATH");

        let output = Command::new("cargo")
            .args(env::args().skip(1))
            .env("PATH", remainder)
            .output()
            .context("Starting cargo failed")?;

        if !output.status.success() {
            std::process::exit(output.status.code().unwrap());
        }

        let mut stdout_writer =
            BufWriter::new(File::create(stdout_path).context("Failed to create stdout path")?);

        // Copy over the artifacts
        for message in Message::parse_stream(&*output.stdout) {
            let patched_message =
                match message.context("Failed to parse message coming from cargo")? {
                    cargo_metadata::Message::CompilerArtifact(mut artifact) => {
                        let crates_types = artifact.target.crate_types.clone();
                        for (pos, artifact_type) in crates_types.into_iter().enumerate() {
                            if artifact_type != "lib" {
                                let original_path = artifact.filenames[pos].clone();
                                let new_path = cache_path.join(
                                    original_path
                                        .file_name()
                                        .expect("Path from cargo should have a filename"),
                                );
                                fs::copy(&original_path, new_path)
                                    .context("Failed to copy the artifact to the cache")?;
                                artifact.filenames[pos] = original_path;
                            }
                        }
                        cargo_metadata::Message::CompilerArtifact(artifact)
                    }
                    message => message,
                };

            let patched_line =
                serde_json::to_string(&patched_message).expect("Failed to re-seralize");
            println!("{}", patched_line);
            stdout_writer
                .write_all(patched_line.as_bytes())
                .context("Failed to write to stdout file")?;
            stdout_writer
                .write_all(b"\n")
                .context("Failed to write to stdout file")?;
        }

        File::create(stderr_path)
            .and_then(|mut file| file.write_all(&output.stderr))
            .context("Failed to write to stderr file")?;
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("💥 Cargo mock failed");
        for cause in e.chain().collect::<Vec<_>>().iter() {
            eprintln!("  Caused by: {}", cause);
        }
        std::process::exit(1);
    }
}
