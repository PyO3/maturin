#[uniffi::export]
fn add(a: u32, b: u32) -> u32 {
    a + b
}

struct NumbersToAdd {
    numbers: Vec<i32>,
}

uniffi::include_scaffolding!("math");
