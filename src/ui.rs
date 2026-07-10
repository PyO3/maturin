//! User-facing output facade.
//!
//! This is the single seam for later `--quiet` and JSON output support, introduced by
//! the refactor plan item "Route user-facing output through a small facade".

macro_rules! status {
    ($($arg:tt)*) => {
        eprintln!($($arg)*)
    };
}

macro_rules! warning {
    ($($arg:tt)*) => {
        eprintln!($($arg)*)
    };
}

pub(crate) use status;
pub(crate) use warning;
