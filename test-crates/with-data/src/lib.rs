use std::ffi::CString;
use std::os::raw::{c_char, c_int};

#[no_mangle]
pub unsafe extern "C" fn say_hello() -> *const c_char {
    CString::new("hello").unwrap().into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn one() -> c_int {
    1
}
