use crate::target::get_host_target;
use crate::Target;
use anyhow::Result;

pub fn is_cross_compiling(target: &Target) -> Result<bool> {
    let target_triple = target.target_triple();
    let host = get_host_target()?;
    if target_triple == host {
        // Not cross-compiling
        return Ok(false);
    }

    if target_triple == "x86_64-apple-darwin" && host == "aarch64-apple-darwin" {
        // Not cross-compiling to compile for x86-64 Python from macOS arm64
        return Ok(false);
    }
    if target_triple == "aarch64-apple-darwin" && host == "x86_64-apple-darwin" {
        // Not cross-compiling to compile for arm64 Python from macOS x86_64
        return Ok(false);
    }

    if let Some(target_without_env) = target_triple
        .rfind('-')
        .map(|index| &target_triple[0..index])
    {
        if host.starts_with(target_without_env) {
            // Not cross-compiling if arch-vendor-os is all the same
            // e.g. x86_64-unknown-linux-musl on x86_64-unknown-linux-gnu host
            return Ok(false);
        }
    }

    Ok(true)
}
