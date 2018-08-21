use reqwest::Url;

/// A pip registry such as pypi or testpypi with associated credentials, used
/// for uploading wheels
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Registry {
    /// The username
    pub username: String,
    /// The password
    pub password: String,
    /// The url endpoint for legacy uploading
    pub url: Url,
}

impl Registry {
    /// Creates a new registry
    pub fn new(username: String, password: String, url: Url) -> Registry {
        Registry {
            username,
            password,
            url,
        }
    }
}
