#[uniffi::export]
pub fn get_status() -> mylib::Status {
    mylib::get_status()
}

uniffi::setup_scaffolding!();
