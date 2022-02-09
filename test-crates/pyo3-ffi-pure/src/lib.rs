use pyo3_ffi::*;
use std::os::raw::c_char;

#[allow(non_snake_case)]
#[no_mangle]
pub unsafe extern "C" fn PyInit_pyo3_ffi_pure() -> *mut PyObject {
    let module_name = "pyo3_ffi_pure\0".as_ptr() as *const c_char;
    let init = PyModuleDef {
        m_base: PyModuleDef_HEAD_INIT,
        m_name: module_name,
        m_doc: std::ptr::null(),
        m_size: 0,
        m_methods: std::ptr::null_mut(),
        m_slots: std::ptr::null_mut(),
        m_traverse: None,
        m_clear: None,
        m_free: None,
    };
    let mptr = PyModule_Create(Box::into_raw(Box::new(init)));

    let wrapped_sum = PyMethodDef {
        ml_name: "sum\0".as_ptr() as *const c_char,
        ml_meth: Some(std::mem::transmute::<PyCFunctionWithKeywords, PyCFunction>(
            sum,
        )),
        ml_flags: METH_VARARGS,
        ml_doc: std::ptr::null_mut(),
    };
    PyModule_AddObject(
        mptr,
        "sum\0".as_ptr() as *const c_char,
        PyCFunction_NewEx(
            Box::into_raw(Box::new(wrapped_sum)),
            std::ptr::null_mut(),
            PyUnicode_InternFromString(module_name),
        ),
    );

    mptr
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
