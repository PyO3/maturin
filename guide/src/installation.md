# Installation

## Install from package managers

[![Packaging status](https://repology.org/badge/vertical-allrepos/maturin.svg?columns=4)](https://repology.org/project/maturin/versions)

### PyPI

maturin is published as Python binary wheel to PyPI, you can install it using pip:

```bash
pip install maturin
```

### Homebrew

On macOS [maturin is in Homebrew](https://formulae.brew.sh/formula/maturin#default) and you can install maturin from Homebrew:

```bash
brew install maturin
```

### conda

Installing from the `conda-forge` channel can be achieved by adding `conda-forge` to your conda channels with:

```
conda config --add channels conda-forge
conda config --set channel_priority strict
```

Once the `conda-forge` channel has been enabled, `maturin` can be installed with:

```
conda install maturin
```

### Alpine Linux

On Alpine Linux, [maturin is in community repository](https://pkgs.alpinelinux.org/packages?name=maturin&branch=edge&repo=community)
and can be installed with `apk` after [enabling the community repository](https://wiki.alpinelinux.org/wiki/Enable_Community_Repository):

```bash
apk add maturin
```

## Download from GitHub Releases

You can download precompiled maturin binaries from the latest [GitHub Releases](https://github.com/PyO3/maturin/releases/latest).

## Build from source

### crates.io

You can install maturin from [crates.io](https://crates.io/crates/maturin) using cargo:

```bash
cargo install maturin
```

### Git repository

```bash
cargo install --git https://github.com/PyO3/maturin.git maturin
```
