//! To speed up the tests, they are all collected in a single integration-test crate.

mod common;
#[path = "run/develop.rs"]
mod develop;
#[path = "run/environment.rs"]
mod environment;
#[path = "run/errors.rs"]
mod errors;
#[path = "run/integration.rs"]
mod integration;
#[path = "run/pep517.rs"]
mod pep517;
#[path = "run/sdist.rs"]
mod sdist;
#[path = "run/wheel.rs"]
mod wheel;
