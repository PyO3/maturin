use pyo3::ffi::{PyDict_New, PyObject};

#[no_mangle]
#[allow(non_snake_case)]
pub unsafe extern "C" fn PyInit_pyo3_pure() -> *mut PyObject {
    PyDict_New() // Make sure an ffi function is used
}
