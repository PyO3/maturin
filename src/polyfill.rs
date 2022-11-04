use cargo_metadata::{Metadata, MetadataCommand};
use std::process::Stdio;

pub trait MetadataCommandExt {
    /// Runs configured `cargo metadata` and returns parsed `Metadata`.
    /// Inherits stderr from parent process.
    fn exec_inherit_stderr(&self) -> Result<Metadata, cargo_metadata::Error>;
}

impl MetadataCommandExt for MetadataCommand {
    fn exec_inherit_stderr(&self) -> Result<Metadata, cargo_metadata::Error> {
        let mut command = self.cargo_command();
        command.stderr(Stdio::inherit());
        let output = command.output()?;
        if !output.status.success() {
            return Err(cargo_metadata::Error::CargoMetadata {
                stderr: String::from_utf8(output.stderr)?,
            });
        }
        let stdout = std::str::from_utf8(&output.stdout)?
            .lines()
            .find(|line| line.starts_with('{'))
            .ok_or(cargo_metadata::Error::NoJson)?;
        Self::parse(stdout)
    }
}
