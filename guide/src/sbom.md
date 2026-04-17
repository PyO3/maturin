# SBOM (Software Bill of Materials)

Maturin can automatically generate [CycloneDX](https://cyclonedx.org/) SBOMs and
include them in the built wheel under the `.dist-info/sboms/` directory,
following the convention described in [PEP 770](https://peps.python.org/pep-0770/).

## Overview

Three kinds of SBOMs can be included in a wheel:

| Kind | File in wheel | Description |
|---|---|---|
| **Rust** | `<dist-info>/sboms/<crate>.cyclonedx.json` | CycloneDX 1.5 SBOM of the Rust dependency tree, generated via `cargo-cyclonedx`. |
| **Auditwheel** | `<dist-info>/sboms/auditwheel.cdx.json` | CycloneDX 1.4 SBOM listing the OS packages (deb/rpm/apk) that provided shared libraries grafted during `auditwheel` repair. |
| **Custom** | `<dist-info>/sboms/<filename>` | Any additional SBOM files you provide. |

## Requirements

The **Rust** and **Auditwheel** SBOMs require the `sbom` Cargo feature, which
is included in the `full` feature set and enabled by default. If you installed
maturin from PyPI or a pre-built binary, SBOM support is already available.

If you build maturin from source without default features, enable it with:

```bash
cargo install maturin --features sbom
```

**Custom** SBOM includes work regardless of the `sbom` feature.

## Configuration

SBOM generation is configured in the `[tool.maturin.sbom]` section of
`pyproject.toml`:

```toml
[tool.maturin.sbom]
# Generate a CycloneDX SBOM for the Rust dependency tree.
# Defaults to true when the sbom feature is enabled.
rust = true

# Generate a CycloneDX SBOM for external shared libraries grafted during
# auditwheel repair. Defaults to true when repair copies libraries.
auditwheel = true

# Additional SBOM files to include in the wheel.
# Paths are relative to the project root.
include = ["sboms/vendor.cdx.json", "sboms/license-report.spdx.json"]
```

All three keys are optional. When the section is omitted entirely, the defaults
apply (Rust and auditwheel SBOMs are generated automatically).

### Disabling SBOM generation

To disable Rust SBOM generation:

```toml
[tool.maturin.sbom]
rust = false
```

To disable the auditwheel SBOM:

```toml
[tool.maturin.sbom]
auditwheel = false
```

## Rust SBOM

When enabled, maturin uses [`cargo-cyclonedx`](https://github.com/CycloneDX/cyclonedx-rust-cargo)
to produce a CycloneDX 1.5 SBOM that captures the full transitive dependency
graph of the crate being built. The SBOM is generated once per build and reused
across all wheels (the Rust dependency graph does not change per Python
interpreter).

The output file is named `<crate_name>.cyclonedx.json` and placed in the
`.dist-info/sboms/` directory.

## Auditwheel SBOM

On Linux, when maturin repairs a wheel by copying external shared libraries
into it (the `auditwheel = "repair"` mode), it can also generate a CycloneDX
1.4 SBOM that records which OS packages provided those libraries. It queries
the system package manager (`dpkg`, `rpm`, or `apk`) to determine the package
name, version, and PURL for each grafted library.

The output file is named `auditwheel.cdx.json` and placed in the
`.dist-info/sboms/` directory.

## Custom SBOM includes

You can bundle arbitrary SBOM files (any format) into the wheel using the
`include` option. Paths are resolved relative to the project root and must
not escape it. Each included file must have a unique filename.

```toml
[tool.maturin.sbom]
include = [
    "sboms/third-party.cdx.json",
    "sboms/licenses.spdx.json",
]
```

### CLI argument

Additional SBOM files can also be included via the `--sbom-include` CLI
argument, which is useful when you only want to include SBOMs in certain
environments (e.g. CI) without modifying `pyproject.toml`:

```bash
maturin build --sbom-include sboms/ci-report.cdx.json

# Multiple files at once
maturin build --sbom-include sboms/a.cdx.json sboms/b.cdx.json

# Or repeated flags
maturin build --sbom-include sboms/a.cdx.json --sbom-include sboms/b.cdx.json
```

Paths from `--sbom-include` are merged with any `include` paths in
`pyproject.toml` and deduplicated.

## Inspecting SBOMs in a wheel

After building a wheel, you can inspect the included SBOMs by unzipping it:

```bash
maturin build --release
unzip -l target/wheels/*.whl | grep sboms/
```

Or extract a specific SBOM:

```bash
unzip -p target/wheels/*.whl '*.dist-info/sboms/*.json' | python -m json.tool
```
