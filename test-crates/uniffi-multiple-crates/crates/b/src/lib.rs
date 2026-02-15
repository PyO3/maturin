uniffi::setup_scaffolding!();

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RandomEnum {
    A = 0,
    B = 1,
}
