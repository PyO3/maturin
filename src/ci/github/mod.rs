mod render;
mod resolve;
mod yaml;

pub(crate) use render::{generate_github, generate_github_from_cli};
pub(crate) use resolve::resolve_config;

#[cfg(test)]
mod tests;
