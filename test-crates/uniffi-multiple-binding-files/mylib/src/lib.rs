#[derive(uniffi::Enum)]
pub enum Status {
    Running,
    Complete,
}

pub fn get_status() -> Status {
    Status::Complete
}

uniffi::setup_scaffolding!();
