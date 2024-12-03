use std::fmt::{Display, Formatter};

/// The name and version of the bindings crate
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    /// The name of the bindings crate, `pyo3`, `rust-cpython` or `uniffi`
    pub name: String,
    /// bindings crate version
    pub version: semver::Version,
}

impl Bindings {
    /// Returns the minimum python minor version supported
    pub fn minimal_python_minor_version(&self) -> usize {
        use crate::python_interpreter::MINIMUM_PYTHON_MINOR;

        match self.name.as_str() {
            "pyo3" | "pyo3-ffi" => {
                let major_version = self.version.major;
                let minor_version = self.version.minor;
                // N.B. must check large minor versions first
                if (major_version, minor_version) >= (0, 16) {
                    7
                } else {
                    MINIMUM_PYTHON_MINOR
                }
            }
            _ => MINIMUM_PYTHON_MINOR,
        }
    }

    /// Returns the minimum PyPy minor version supported
    pub fn minimal_pypy_minor_version(&self) -> usize {
        use crate::python_interpreter::MINIMUM_PYPY_MINOR;

        match self.name.as_str() {
            "pyo3" | "pyo3-ffi" => {
                let major_version = self.version.major;
                let minor_version = self.version.minor;
                // N.B. must check large minor versions first
                if (major_version, minor_version) >= (0, 23) {
                    9
                } else if (major_version, minor_version) >= (0, 14) {
                    7
                } else {
                    MINIMUM_PYPY_MINOR
                }
            }
            _ => MINIMUM_PYPY_MINOR,
        }
    }
}

/// The way the rust code is used in the wheel
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeModel {
    /// A rust binary to be shipped a python package
    Bin(Option<Bindings>),
    /// A native module with pyo3 or rust-cpython bindings.
    Bindings(Bindings),
    /// `Bindings`, but specifically for pyo3 with feature flags that allow building a single wheel
    /// for all cpython versions (pypy & graalpy still need multiple versions).
    /// The numbers are the minimum major and minor version
    BindingsAbi3(u8, u8),
    /// A native module with c bindings, i.e. `#[no_mangle] extern "C" <some item>`
    Cffi,
    /// A native module generated from uniffi
    UniFfi,
}

impl BridgeModel {
    /// Returns the bindings
    pub fn bindings(&self) -> Option<&Bindings> {
        match self {
            BridgeModel::Bin(Some(bindings)) => Some(bindings),
            BridgeModel::Bindings(bindings) => Some(bindings),
            _ => None,
        }
    }

    /// Returns the name of the bindings crate
    pub fn unwrap_bindings_name(&self) -> &str {
        match self {
            BridgeModel::Bindings(bindings) => &bindings.name,
            _ => panic!("Expected Bindings"),
        }
    }

    /// Test whether this is using a specific bindings crate
    pub fn is_bindings(&self, name: &str) -> bool {
        match self {
            BridgeModel::Bin(Some(bindings)) => bindings.name == name,
            BridgeModel::Bindings(bindings) => bindings.name == name,
            _ => false,
        }
    }

    /// Test whether this is bin bindings
    pub fn is_bin(&self) -> bool {
        matches!(self, BridgeModel::Bin(_))
    }
}

impl Display for BridgeModel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeModel::Bin(Some(bindings)) => write!(f, "{} bin", bindings.name),
            BridgeModel::Bin(None) => write!(f, "bin"),
            BridgeModel::Bindings(bindings) => write!(f, "{}", bindings.name),
            BridgeModel::BindingsAbi3(..) => write!(f, "pyo3"),
            BridgeModel::Cffi => write!(f, "cffi"),
            BridgeModel::UniFfi => write!(f, "uniffi"),
        }
    }
}
