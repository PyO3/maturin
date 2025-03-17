use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

use crate::python_interpreter::{MINIMUM_PYPY_MINOR, MINIMUM_PYTHON_MINOR};

/// pyo3 binding crate
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyO3Crate {
    /// pyo3
    PyO3,
    /// pyo3-ffi
    PyO3Ffi,
}

impl PyO3Crate {
    /// Returns the name of the crate as a string
    pub fn as_str(&self) -> &str {
        match self {
            PyO3Crate::PyO3 => "pyo3",
            PyO3Crate::PyO3Ffi => "pyo3-ffi",
        }
    }
}

impl Display for PyO3Crate {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for PyO3Crate {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pyo3" => Ok(PyO3Crate::PyO3),
            "pyo3-ffi" => Ok(PyO3Crate::PyO3Ffi),
            _ => anyhow::bail!("unknown binding crate: {}", s),
        }
    }
}

/// The name and version of the pyo3 bindings crate
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyO3 {
    /// The name of the bindings crate, `pyo3` or `uniffi`
    pub crate_name: PyO3Crate,
    /// pyo3 bindings crate version
    pub version: semver::Version,
}

impl PyO3 {
    /// Returns the minimum python minor version supported
    fn minimal_python_minor_version(&self) -> usize {
        use crate::python_interpreter::MINIMUM_PYTHON_MINOR;

        let major_version = self.version.major;
        let minor_version = self.version.minor;
        // N.B. must check large minor versions first
        if (major_version, minor_version) >= (0, 16) {
            7
        } else {
            MINIMUM_PYTHON_MINOR
        }
    }

    /// Returns the minimum PyPy minor version supported
    fn minimal_pypy_minor_version(&self) -> usize {
        use crate::python_interpreter::MINIMUM_PYPY_MINOR;

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

    /// free-threaded Python support
    fn supports_free_threaded(&self) -> bool {
        let major_version = self.version.major;
        let minor_version = self.version.minor;
        (major_version, minor_version) >= (0, 23)
    }
}

/// The way the rust code is used in the wheel
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeModel {
    /// A rust binary to be shipped a python package
    Bin(Option<PyO3>),
    /// A native module with pyo3 bindings.
    PyO3(PyO3),
    /// `Bindings`, but specifically for pyo3 with feature flags that allow building a single wheel
    /// for all cpython versions (pypy & graalpy still need multiple versions).
    PyO3Abi3 {
        /// The bindings crate
        bindings: PyO3,
        /// Minimal abi3 major version
        major: u8,
        /// Minimal abi3 minor version
        minor: u8,
    },
    /// A native module with c bindings, i.e. `#[no_mangle] extern "C" <some item>`
    Cffi,
    /// A native module generated from uniffi
    UniFfi,
}

impl BridgeModel {
    /// Returns the pyo3 bindings
    pub fn pyo3(&self) -> Option<&PyO3> {
        match self {
            BridgeModel::Bin(Some(bindings)) => Some(bindings),
            BridgeModel::PyO3(bindings) => Some(bindings),
            BridgeModel::PyO3Abi3 { bindings, .. } => Some(bindings),
            _ => None,
        }
    }

    /// Test whether this is using pyo3/pyo3-ffi
    pub fn is_pyo3(&self) -> bool {
        matches!(
            self,
            BridgeModel::PyO3(_) | BridgeModel::PyO3Abi3 { .. } | BridgeModel::Bin(Some(_))
        )
    }

    /// Test whether this is using a specific pyo3 crate
    pub fn is_pyo3_crate(&self, name: PyO3Crate) -> bool {
        match self {
            BridgeModel::Bin(Some(bindings)) => bindings.crate_name == name,
            BridgeModel::PyO3(bindings) => bindings.crate_name == name,
            BridgeModel::PyO3Abi3 { bindings, .. } => bindings.crate_name == name,
            _ => false,
        }
    }

    /// Test whether this is bin bindings
    pub fn is_bin(&self) -> bool {
        matches!(self, BridgeModel::Bin(_))
    }

    /// Returns the minimum python minor version supported
    pub fn minimal_python_minor_version(&self) -> usize {
        match self {
            BridgeModel::Bin(Some(bindings)) | BridgeModel::PyO3(bindings) => {
                bindings.minimal_python_minor_version()
            }
            BridgeModel::PyO3Abi3 {
                bindings,
                minor: abi3_minor,
                ..
            } => {
                let bindings_minor = bindings.minimal_python_minor_version();
                bindings_minor.max(*abi3_minor as usize)
            }
            BridgeModel::Bin(None) | BridgeModel::Cffi | BridgeModel::UniFfi => {
                MINIMUM_PYTHON_MINOR
            }
        }
    }

    /// Returns the minimum PyPy minor version supported
    pub fn minimal_pypy_minor_version(&self) -> usize {
        match self.pyo3() {
            Some(bindings) => bindings.minimal_pypy_minor_version(),
            None => MINIMUM_PYPY_MINOR,
        }
    }

    /// free-threaded Python support
    pub fn supports_free_threaded(&self) -> bool {
        match self {
            BridgeModel::Bin(Some(bindings))
            | BridgeModel::PyO3(bindings)
            | BridgeModel::PyO3Abi3 { bindings, .. } => bindings.supports_free_threaded(),
            BridgeModel::Bin(None) => true,
            BridgeModel::Cffi | BridgeModel::UniFfi => false,
        }
    }
}

impl Display for BridgeModel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeModel::Bin(Some(bindings)) => write!(f, "{} bin", bindings.crate_name),
            BridgeModel::Bin(None) => write!(f, "bin"),
            BridgeModel::PyO3(bindings) => write!(f, "{}", bindings.crate_name),
            BridgeModel::PyO3Abi3 { bindings, .. } => write!(f, "{}", bindings.crate_name),
            BridgeModel::Cffi => write!(f, "cffi"),
            BridgeModel::UniFfi => write!(f, "uniffi"),
        }
    }
}
