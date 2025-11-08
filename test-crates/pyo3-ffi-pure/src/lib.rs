use std::ptr;

use pyo3_ffi::*;

static mut MODULE_DEF: PyModuleDef = PyModuleDef {
    m_base: PyModuleDef_HEAD_INIT,
    m_name: c"string_sum".as_ptr(),
    m_doc: c"A Python module written in Rust.".as_ptr(),
    m_size: 0,
    m_methods: std::ptr::addr_of_mut!(METHODS).cast(),
    m_slots: std::ptr::addr_of_mut!(SLOTS).cast(),
    m_traverse: None,
    m_clear: None,
    m_free: None,
};

static mut METHODS: [PyMethodDef; 2] = [
    PyMethodDef {
        ml_name: c"sum".as_ptr(),
        ml_meth: PyMethodDefPointer {
            PyCFunctionWithKeywords: sum,
        },
        ml_flags: METH_VARARGS | METH_KEYWORDS,
        ml_doc: c"returns the sum of two integers".as_ptr(),
    },
    // A zeroed PyMethodDef to mark the end of the array.
    PyMethodDef::zeroed(),
];

const SLOTS_LEN: usize =
    1 + if cfg!(Py_3_12) { 1 } else { 0 } + if cfg!(Py_GIL_DISABLED) { 1 } else { 0 };

static mut SLOTS: [PyModuleDef_Slot; SLOTS_LEN] = [
    // NB: only include this slot if the module does not store any global state in `static` variables
    // or other data which could cross between subinterpreters
    #[cfg(Py_3_12)]
    PyModuleDef_Slot {
        slot: Py_mod_multiple_interpreters,
        value: Py_MOD_PER_INTERPRETER_GIL_SUPPORTED,
    },
    // NB: only include this slot if the module does not depend on the GIL for thread safety
    #[cfg(Py_GIL_DISABLED)]
    PyModuleDef_Slot {
        slot: Py_mod_gil,
        value: Py_MOD_GIL_NOT_USED,
    },
    PyModuleDef_Slot {
        slot: 0,
        value: ptr::null_mut(),
    },
];

// The module initialization function, which must be named `PyInit_<your_module>`.
#[allow(non_snake_case)]
#[no_mangle]
pub unsafe extern "C" fn PyInit_pyo3_ffi_pure() -> *mut PyObject {
    PyModuleDef_Init(ptr::addr_of_mut!(MODULE_DEF))
}

#[no_mangle]
pub unsafe extern "C" fn sum(
    _self: *mut PyObject,
    args: *mut PyObject,
    _kwds: *mut PyObject,
) -> *mut PyObject {
    // this is a minimal test of compilation, not good example code
    let val_a = PyTuple_GetItem(args, 0);
    let val_b = PyTuple_GetItem(args, 1);
    let res: i64 = PyLong_AsLongLong(val_a) + PyLong_AsLongLong(val_b);
    PyLong_FromLongLong(res)
}
