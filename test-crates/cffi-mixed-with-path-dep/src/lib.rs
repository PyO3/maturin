use some_path_dep::{add, is_sum};

#[no_mangle]
pub unsafe extern "C" fn get_21() -> usize {
    21
}

#[no_mangle]
pub unsafe extern "C" fn add_21(num: usize) -> usize {
    add(num, get_21())
}

#[no_mangle]
pub unsafe extern "C" fn is_half(a: usize, b: usize) -> bool {
    is_sum(a, a, b)
}
