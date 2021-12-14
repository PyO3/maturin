# Tutorial

In this tutorial we will wrap a version of [the guessing game from The Rust
Book](https://doc.rust-lang.org/book/ch02-00-guessing-game-tutorial.html) to
run in Python using pyo3.

## Create a new Rust project

First, create a new Rust library project using `cargo new --lib --edition 2018
guessing-game`. This will create a directory with the following structure.

```ignore
guessing-game/
├── Cargo.toml
└── src
    └── lib.rs
```

Edit `Cargo.toml` to configure the project and module name, and add the
dependencies (`rand` and `pyo3`). Configure `pyo3` with additional features to
make an extension module compatible with multiple Python versions using the
stable ABI (`abi3`).

```toml
[package]
name = "guessing-game"
version = "0.1.0"
edition = "2018"

[lib]
name = "guessing_game"
# "cdylib" is necessary to produce a shared library for Python to import from.
crate-type = ["cdylib"]

[dependencies]
rand = "0.8.4"

[dependencies.pyo3]
version = "0.14.5"
# "extension-module" tells pyo3 we want to build an extension module (skips linking against libpython.so)
# "abi3-py36" tells pyo3 (and maturin) to build using the stable ABI with minimum Python version 3.6
features = ["extension-module", "abi3-py36"]
```

### Use `maturin new`

New projects can also be quickly created using the `maturin new` command:

```bash
USAGE:
    maturin new [FLAGS] [OPTIONS] <path>

FLAGS:
    -h, --help       Prints help information
        --mixed      Use mixed Rust/Python project layout
    -V, --version    Prints version information

OPTIONS:
    -b, --bindings <bindings>    Which kind of bindings to use [possible values: pyo3, rust-cpython, cffi, bin]
        --name <name>            Set the resulting package name, defaults to the directory name

ARGS:
    <path>    Project path
```

The above process can be achieved by running `maturin new -b pyo3 guessing_game`
then edit `Cargo.toml` to add `abi3-py36` feature.

## Install and configure maturin (in a virtual environment)

Create a virtual environment and install maturin. Note maturin has minimal
dependencies!

```shell
ferris@rustbox [~/src/rust/guessing-game] % python3 -m venv .venv
ferris@rustbox [~/src/rust/guessing-game] % source .venv/bin/activate
(.venv) ferris@rustbox [~/src/rust/guessing-game] % pip install -U pip maturin
(.venv) ferris@rustbox [~/src/rust/guessing-game] % pip freeze
maturin==0.11.5
toml==0.10.2
```

maturin is configured in `pyproject.toml` as introduced by [PEP
518](https://www.python.org/dev/peps/pep-0518/).  This file lives in the root
of your project tree:

```ignore
guessing-game/
├── Cargo.toml
├── pyproject.toml  #  <<< add this file
└── src
    └── lib.rs
```

Configuration in this file is quite simple for most projects. You just need to
indicate maturin as a requirement (and restrict the version) and as the
build-backend (Python supports a number of build-backends since [PEP
517](https://www.python.org/dev/peps/pep-0517/)).

```toml
[build-system]
requires = ["maturin>=0.12,<0.13"]
build-backend = "maturin"
```

Various other tools may also be configured in `pyproject.toml` and the Python
community seems to be consolidating declarative configuration in this file.

## Program the guessing game in Rust

When you create a `lib` projectg with `cargo new` it creates a file
`src/lib.rs` with some default code. Edit that file and replace the default
code with the code below. As mentioned, we will implement a slightly
modified version of [the guessing game from The Rust
Book](https://doc.rust-lang.org/book/ch02-00-guessing-game-tutorial.html).
Instead of implementing as a `bin` crate, we're using a `lib` and will expose
the main logic as a Python function.

```rust,no_run
use pyo3::prelude::*;
use rand::Rng;
use std::cmp::Ordering;
use std::io;

#[pyfunction]
fn guess_the_number() {
    println!("Guess the number!");

    let secret_number = rand::thread_rng().gen_range(1..101);

    loop {
        println!("Please input your guess.");

        let mut guess = String::new();

        io::stdin()
            .read_line(&mut guess)
            .expect("Failed to read line");

        let guess: u32 = match guess.trim().parse() {
            Ok(num) => num,
            Err(_) => continue,
        };

        println!("You guessed: {}", guess);

        match guess.cmp(&secret_number) {
            Ordering::Less => println!("Too small!"),
            Ordering::Greater => println!("Too big!"),
            Ordering::Equal => {
                println!("You win!");
                break;
            }
        }
    }
}

/// A Python module implemented in Rust. The name of this function must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
fn guessing_game(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(guess_the_number, m)?)?;

    Ok(())
}
```

Thanks to pyo3, there's very little difference between this and the example in
The Rust Book. All we had to do was:
1. Include the pyo3 prelude
2. Add `#[pyfunction]` to our function
3. Add the `#[pymodule]` block to expose the function as part of a Python module

Refer to the [pyo3 User Guide](https://pyo3.rs/) for more information on using
pyo3. It can do a lot more!

## Build and install the module with `maturin develop`

Note that *this is just a Rust project* at this point, and with few exceptions
you can build it as you'd expect using `cargo build`. maturin helps with this,
however, adding some platform-specific build configuration and ultimately
packaging the binary results as a wheel (a `.whl` file, which is an archive of
compiled components suitable for installation with `pip`, the Python package
manager).

So let's use maturin to build and install in our current environment.

```shell
(.venv) ferris@rustbox [~/src/rust/guessing-game] % maturin develop
🔗 Found pyo3 bindings with abi3 support for Python ≥ 3.6
🐍 Not using a specific python interpreter (With abi3, an interpreter is only required on windows)
   Compiling libc v0.2.105
   Compiling proc-macro2 v1.0.32
   Compiling cfg-if v1.0.0
   Compiling unicode-xid v0.2.2
   Compiling syn v1.0.81
   Compiling proc-macro-hack v0.5.19
   Compiling pyo3-build-config v0.14.5
   Compiling once_cell v1.8.0
   Compiling parking_lot_core v0.8.5
   Compiling smallvec v1.7.0
   Compiling scopeguard v1.1.0
   Compiling unindent v0.1.7
   Compiling ppv-lite86 v0.2.15
   Compiling instant v0.1.12
   Compiling lock_api v0.4.5
   Compiling getrandom v0.2.3
   Compiling quote v1.0.10
   Compiling rand_core v0.6.3
   Compiling parking_lot v0.11.2
   Compiling paste-impl v0.1.18
   Compiling rand_chacha v0.3.1
   Compiling pyo3 v0.14.5
   Compiling rand v0.8.4
   Compiling paste v0.1.18
   Compiling pyo3-macros-backend v0.14.5
   Compiling indoc-impl v0.3.6
   Compiling indoc v0.3.6
   Compiling pyo3-macros v0.14.5
   Compiling guessing-game v0.1.0 (/Users/ferris/src/rust/guessing-game)
    Finished dev [unoptimized + debuginfo] target(s) in 13.31s
```

Your `guessing_game` module should now be available in your current virtual
environment. Go ahead and play a few games!
```shell
(.venv) ferris@rustbox [~/src/rust/guessing-game] % python
Python 3.9.6 (default, Aug 25 2021, 16:04:27)
[Clang 12.0.5 (clang-1205.0.22.9)] on darwin
Type "help", "copyright", "credits" or "license" for more information.
>>> import guessing_game
>>> guessing_game.guess_the_number()
Guess the number!
Please input your guess.
42
You guessed: 42
Too small!
Please input your guess.
80
You guessed: 80
Too big!
Please input your guess.
50
You guessed: 50
Too small!
Please input your guess.
60
You guessed: 60
Too big!
Please input your guess.
55
You guessed: 55
You win!
```

## Create a wheel for distribution

`maturin develop` actually skips the wheel generation part and installs
directly in the current environment. `maturin build` on the other hand will
produce a wheel you can distribute. Note the wheel contains "tags" in its
filename that correspond to supported Python versions, platforms, and/or
architectures, so yours might look a little different. If you want to
distribute broadly, you may need to build on multiple platforms and use a
[`manylinux`](https://github.com/pypa/manylinux) Docker container to build
wheels compatible with a wide range of Linux distros.

```shell
(.venv) ferris@rustbox [~/src/rust/guessing-game] % maturin build
🔗 Found pyo3 bindings with abi3 support for Python ≥ 3.6
🐍 Not using a specific python interpreter (With abi3, an interpreter is only required on windows)
📦 Built source distribution to /Users/ferris/src/rust/guessing-game/target/wheels/guessing_game-0.1.0.tar.gz
   Compiling pyo3-build-config v0.14.5
   Compiling pyo3-macros-backend v0.14.5
   Compiling pyo3 v0.14.5
   Compiling pyo3-macros v0.14.5
   Compiling guessing-game v0.1.0 (/Users/ferris/src/rust/guessing-game)
    Finished dev [unoptimized + debuginfo] target(s) in 7.32s
📦 Built wheel for abi3 Python ≥ 3.6 to /Users/ferris/src/rust/guessing-game/target/wheels/guessing_game-0.1.0-cp36-abi3-macosx_10_7_x86_64.whl
```

maturin can even publish wheels directly to [PyPI](https://pypi.org) with
`maturin publish`!

## Summary
Congratulations! You successfully created a Python module implemented entirely
in Rust thanks to pyo3 and maturin.

This demonstrates how easy it is to get started with maturin, but keep reading
to learn more about all the additional features.
