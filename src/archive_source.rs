use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum ArchiveSource {
    Generated(GeneratedSourceData),
    File(FileSourceData),
}

impl ArchiveSource {
    pub(crate) fn executable(&self) -> bool {
        match self {
            Self::Generated(source) => source.executable,
            Self::File(source) => source.executable,
        }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::Generated(source) => source.path.as_deref(),
            Self::File(source) => Some(&source.path),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedSourceData {
    pub(crate) data: Vec<u8>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) executable: bool,
}

#[derive(Debug, Clone)]
pub struct FileSourceData {
    pub(crate) path: PathBuf,
    pub(crate) executable: bool,
}
