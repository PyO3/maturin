use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::collections::hash_map::VacantEntry;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use anyhow::bail;
use ignore::overrides::Override;
#[cfg(test)]
use indexmap::IndexMap;
use once_cell::unsync::OnceCell;
use same_file::is_same_file;
use tempfile::TempDir;
use tempfile::tempdir;
use tracing::debug;

use crate::Metadata24;
use crate::archive_source::ArchiveSource;
use crate::archive_source::FileSourceData;
use crate::archive_source::GeneratedSourceData;

use super::ModuleWriter;
use super::ModuleWriterInternal;
use super::PathWriter;
use super::SDistWriter;
use super::WheelWriter;
#[cfg(test)]
use super::mock_writer::MockWriter;
use super::write_dist_info;

/// A 'virtual' module writer that tracks entries to be added to the archive
/// and writes them to the underlying archive at the end.
/// This struct provides 2 primary functions:
/// 1. Serves as the single point of enforcement to decide which entries are included
///    in the archive
/// 2. Ensure that the entries are written to the underlying archive in a consistent
///    order for build reproducibility
pub struct VirtualWriter<W> {
    inner: W,
    tracker: HashMap<PathBuf, ArchiveSource>,
    excludes: Override,
    target_exclusion_warning_emitted: bool,
    temp_dir: OnceCell<Rc<TempDir>>,
}

impl<W: ModuleWriterInternal> VirtualWriter<W> {
    /// Construct a new [VirtualWriter] wrapping the provided inner writer and
    /// using the `excludes` for filtering files
    pub fn new(inner: W, excludes: Override) -> Self {
        Self {
            inner,
            tracker: HashMap::new(),
            excludes,
            target_exclusion_warning_emitted: false,
            temp_dir: OnceCell::new(),
        }
    }

    /// Provides a temp dir that can contain files that will be added to the archive later
    pub(crate) fn temp_dir(&self) -> Result<Rc<TempDir>> {
        self.temp_dir
            .get_or_try_init(|| {
                let temp_dir = tempdir()?;
                Ok(Rc::new(temp_dir))
            })
            .cloned()
    }

    /// Returns `true` if the given path should be excluded
    fn exclude(&self, path: impl AsRef<Path>) -> bool {
        self.excludes.matched(path.as_ref(), false).is_whitelist()
    }

    /// Checks exclusions and previously tracked sources to determine if the
    /// current source should be allowed.
    /// Returns Ok(Some(..)) if the new source should be included, Ok(None) if
    /// the new source should not be included (excluded or duplicate).
    fn get_entry(
        &mut self,
        target: PathBuf,
        source: Option<&Path>,
    ) -> Result<Option<VacantEntry<'_, PathBuf, ArchiveSource>>> {
        if let Some(source) = source {
            if self.exclude(source) {
                return Ok(None);
            }
        }

        if self.exclude(&target) {
            if !self.target_exclusion_warning_emitted {
                self.target_exclusion_warning_emitted = true;
                eprintln!(
                    "⚠️ Warning: A file was excluded from the archive by the target path in the archive\n\
                     ⚠️ instead of the source path on the filesystem. This behavior is deprecated and\n\
                     ⚠️ will be removed in future versions of maturin.",
                );
            }
            debug!("Excluded file {target:?} from archive by target path");
            return Ok(None);
        }

        let entry = match self.tracker.entry(target.clone()) {
            Entry::Vacant(entry) => Some(entry),
            Entry::Occupied(entry) => {
                match (entry.get().path(), source) {
                    (None, None) => {
                        bail!(
                            "Generated file {} was already added, can't add it again",
                            target.display()
                        );
                    }
                    (Some(previous_source), None) => {
                        bail!(
                            "File {} was already added from {}, can't overwrite with generated file",
                            target.display(),
                            previous_source.display()
                        )
                    }
                    (None, Some(source)) => {
                        bail!(
                            "Generated file {} was already added, can't overwrite it with {}",
                            target.display(),
                            source.display()
                        );
                    }
                    (Some(previous_source), Some(source)) => {
                        if is_same_file(source, previous_source).unwrap_or(false) {
                            // Ignore identical duplicate files
                            None
                        } else {
                            bail!(
                                "File {} was already added from {}, can't add it from {}",
                                target.display(),
                                previous_source.display(),
                                source.display()
                            );
                        }
                    }
                }
            }
        };

        Ok(entry)
    }

    pub(crate) fn add_entry(
        &mut self,
        target: impl AsRef<Path>,
        source: ArchiveSource,
    ) -> Result<()> {
        let target = target.as_ref();
        if let Some(entry) = self.get_entry(target.to_path_buf(), source.path())? {
            debug!("Tracked entry {target:?}");
            entry.insert(source);
        }
        Ok(())
    }

    /// Actually write the entries to the underlying archive using the provided comparator
    /// to order the entries
    fn finish_internal(
        mut self,
        comparator: &mut impl FnMut(&PathBuf, &PathBuf) -> Ordering,
    ) -> Result<W> {
        let mut entries: Vec<_> = self.tracker.into_iter().collect();
        entries.sort_unstable_by(|(p1, _), (p2, _)| comparator(p1, p2));

        for (target, entry) in entries {
            self.inner.add_entry(target, entry)?;
        }

        Ok(self.inner)
    }
}

impl<W: ModuleWriterInternal> super::private::Sealed for VirtualWriter<W> {}

impl<W: ModuleWriterInternal> ModuleWriter for VirtualWriter<W> {
    fn add_bytes(
        &mut self,
        target: impl AsRef<Path>,
        source: Option<&Path>,
        data: impl Into<Vec<u8>>,
        executable: bool,
    ) -> Result<()> {
        let source = ArchiveSource::Generated(GeneratedSourceData {
            data: data.into(),
            path: source.map(ToOwned::to_owned),
            executable,
        });
        self.add_entry(target, source)
    }

    fn add_file(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
        executable: bool,
    ) -> Result<()> {
        let source = ArchiveSource::File(FileSourceData {
            path: source.as_ref().to_path_buf(),
            executable,
        });
        self.add_entry(target, source)
    }
}

impl VirtualWriter<PathWriter> {
    /// Commit the tracked entries to the underlying [PathWriter]
    pub fn finish(self) -> Result<()> {
        let mut comparator = PathBuf::cmp;
        let _inner = self.finish_internal(&mut comparator)?;
        Ok(())
    }
}

impl VirtualWriter<SDistWriter> {
    /// Commit the tracked entries to the underlying [SDistWriter]
    pub fn finish(self, pkg_info_path: &Path) -> Result<PathBuf> {
        let mut comparator = self.inner.file_ordering(pkg_info_path);
        let inner = self.finish_internal(&mut comparator)?;
        let path = inner.finish()?;
        Ok(path)
    }
}

impl VirtualWriter<WheelWriter> {
    /// Write the .dist-info for the wheel and commit the tracked entries
    /// to the underlying [WheelWriter]
    pub fn finish(
        mut self,
        metadata24: &Metadata24,
        pyproject_dir: &Path,
        tags: &[String],
    ) -> Result<PathBuf> {
        let dist_info_dir = write_dist_info(&mut self, pyproject_dir, metadata24, tags)?;
        let mut comparator = self.inner.file_ordering(&dist_info_dir);
        let inner = self.finish_internal(&mut comparator)?;
        inner.finish(&dist_info_dir)
    }
}

#[cfg(test)]
impl VirtualWriter<MockWriter> {
    /// Commit the tracked entries to the underlying [MockWriter]
    pub fn finish(self) -> Result<IndexMap<PathBuf, Vec<u8>>> {
        let mut comparator = PathBuf::cmp;
        let inner = self.finish_internal(&mut comparator)?;
        Ok(inner.finish())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Result;
    use ignore::overrides::Override;
    use ignore::overrides::OverrideBuilder;
    use insta::assert_snapshot;
    use itertools::Itertools as _;
    use tempfile::TempDir;

    use crate::ModuleWriter;
    use crate::module_writer::EMPTY;
    use crate::module_writer::mock_writer::MockWriter;

    use super::VirtualWriter;

    #[test]
    fn virtual_writer_no_excludes() -> Result<()> {
        let mut writer = VirtualWriter::new(MockWriter::default(), Override::empty());

        assert!(writer.tracker.is_empty());
        writer.add_empty_file("test")?;
        assert_eq!(writer.tracker.len(), 1);
        writer.finish()?;
        Ok(())
    }

    #[test]
    fn virtual_writer_excludes() -> Result<()> {
        // A test filter
        let tmp_dir = TempDir::new()?;
        let mut excludes = OverrideBuilder::new(&tmp_dir);
        excludes.add("test*")?;
        excludes.add("!test2")?;
        let mut writer = VirtualWriter::new(MockWriter::default(), excludes.build()?);

        writer.add_bytes("test1", Some(Path::new("test1")), EMPTY, true)?;
        writer.add_bytes("test3", Some(Path::new("test3")), EMPTY, true)?;
        assert!(writer.tracker.is_empty());
        writer.add_bytes("yes", Some(Path::new("yes")), EMPTY, true)?;
        assert!(!writer.tracker.is_empty());
        writer.add_bytes("test2", Some(Path::new("test2")), EMPTY, true)?;
        assert_eq!(writer.tracker.len(), 2);
        let files = writer.finish()?;
        tmp_dir.close()?;

        // 'yes' was added before 'test2' above, but the output should be ordered in the end
        assert_snapshot!(files.keys().map(|p| p.to_string_lossy()).collect_vec().join("\n"), @r"
        test2
        yes
        ");
        Ok(())
    }
}
