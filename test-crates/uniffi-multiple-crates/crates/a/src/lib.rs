use b::RandomEnum;

uniffi::setup_scaffolding!();

#[uniffi::export]
pub fn random_enum(e: RandomEnum) -> u8 {
    match e {
        RandomEnum::A => 0,
        RandomEnum::B => 1,
    }
}
