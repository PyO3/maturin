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
    temp_dir: OnceCell<Rc<TempDir>>,
    pending_prepends: HashMap<PathBuf, Vec<u8>>,
}

impl<W: ModuleWriterInternal> VirtualWriter<W> {
    /// Construct a new [VirtualWriter] wrapping the provided inner writer and
    /// using the `excludes` for filtering files
    pub fn new(inner: W, excludes: Override) -> Self {
        Self {
            inner,
            tracker: HashMap::new(),
            excludes,
            temp_dir: OnceCell::new(),
            pending_prepends: HashMap::new(),
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
    pub(crate) fn exclude(&self, path: impl AsRef<Path>) -> bool {
        self.excludes.matched(path.as_ref(), false).is_whitelist()
    }

    /// Returns `true` if the given target path has already been added to the archive
    pub(crate) fn contains_target(&self, target: impl AsRef<Path>) -> bool {
        self.tracker.contains_key(target.as_ref())
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
        if let Some(source) = source
            && self.exclude(source)
        {
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

    /// Adds a file to the archive, bypassing exclusion checks.
    /// This is used for build artifacts (compiled shared libraries) which should
    /// always be included regardless of exclude patterns.
    pub(crate) fn add_file_force(
        &mut self,
        target: impl AsRef<Path>,
        source: impl AsRef<Path>,
        executable: bool,
    ) -> Result<()> {
        let target = target.as_ref();
        let source = source.as_ref();
        debug!("Adding {} from {}", target.display(), source.display());
        let source = ArchiveSource::File(FileSourceData {
            path: source.to_path_buf(),
            executable,
        });
        self.add_entry_force(target, source)
    }

    /// Adds an entry to the archive, bypassing exclusion checks.
    /// This is used for build artifacts and generated files that should
    /// always be included regardless of exclude patterns.
    pub(crate) fn add_entry_force(
        &mut self,
        target: impl AsRef<Path>,
        source: ArchiveSource,
    ) -> Result<()> {
        let target = target.as_ref();
        debug!("Adding {} (forced)", target.display());
        // Directly insert into tracker, bypassing exclusion checks but still detecting duplicates
        if self.tracker.insert(target.to_path_buf(), source).is_some() {
            bail!(
                "File {} overwrote an existing tracked file",
                target.display()
            );
        }
        Ok(())
    }

    /// Register data to be prepended to a file entry.
    ///
    /// The prepend is deferred until `finish_internal()` runs, after all files
    /// have been tracked. This avoids conflicts when `prepend_to` is called
    /// before the target file has been added (e.g., `__init__.py` patching
    /// during wheel repair runs before Python source files are collected).
    ///
    /// For Python files, the data is inserted after any `from __future__`
    /// import lines rather than at byte position 0, to avoid SyntaxErrors.
    pub(crate) fn prepend_to(&mut self, target: impl AsRef<Path>, data: Vec<u8>) -> Result<()> {
        self.pending_prepends
            .entry(target.as_ref().to_path_buf())
            .or_default()
            .extend_from_slice(&data);
        Ok(())
    }

    /// Apply all pending prepends to their corresponding tracked entries.
    ///
    /// For Python files (`.py`), the prepend data is inserted after any
    /// `from __future__` import lines to avoid SyntaxErrors. Targets that
    /// were never tracked (no corresponding `add_file`/`add_bytes`) are
    /// skipped to avoid unexpectedly creating files like `__init__.py` in
    /// namespace packages or binary wheels.
    fn apply_pending_prepends(&mut self) -> Result<()> {
        for (target, prepend_data) in std::mem::take(&mut self.pending_prepends) {
            let is_python = target
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("py"));

            let Some(existing) = self.tracker.remove(&target) else {
                tracing::warn!(
                    "Skipping prepend for {} because it was not tracked. \
                     This avoids creating a new file such as __init__.py unexpectedly.",
                    target.display()
                );
                continue;
            };

            let (file_content, path, executable) = match existing {
                ArchiveSource::Generated(g) => (g.data, g.path, g.executable),
                ArchiveSource::File(f) => (fs_err::read(&f.path)?, Some(f.path), f.executable),
            };

            let insert_pos = if is_python {
                find_python_insertion_point(&file_content)
            } else {
                0
            };

            let mut new_data = Vec::with_capacity(file_content.len() + prepend_data.len());
            new_data.extend_from_slice(&file_content[..insert_pos]);
            new_data.extend_from_slice(&prepend_data);
            new_data.extend_from_slice(&file_content[insert_pos..]);

            self.tracker.insert(
                target,
                ArchiveSource::Generated(GeneratedSourceData {
                    data: new_data,
                    path,
                    executable,
                }),
            );
        }
        Ok(())
    }

    /// Actually write the entries to the underlying archive using the provided comparator
    /// to order the entries
    fn finish_internal(
        mut self,
        comparator: &mut impl FnMut(&PathBuf, &PathBuf) -> Ordering,
    ) -> Result<W> {
        self.apply_pending_prepends()?;

        let mut entries: Vec<_> = self.tracker.into_iter().collect();
        entries.sort_unstable_by(|(p1, _), (p2, _)| comparator(p1, p2));

        for (target, entry) in entries {
            self.inner.add_entry(target, entry)?;
        }

        Ok(self.inner)
    }
}

/// Find the byte position in a Python file where injected code should be inserted.
///
/// Returns the byte offset right after the module preamble, which includes:
/// 1. UTF-8 BOM (if present)
/// 2. Shebang line (if present)
/// 3. Encoding declaration (first or second line, per PEP 263)
/// 4. Module docstring (if present)
/// 5. All `from __future__` import statements
///
/// This ensures injected code (e.g., DLL loader patches) doesn't break:
/// - File encoding detection
/// - The module's `__doc__` attribute
/// - Python's requirement that `from __future__` imports precede other statements
///
/// This uses a line-oriented byte heuristic rather than parsing Python syntax.
fn find_python_insertion_point(content: &[u8]) -> usize {
    let mut pos = 0;

    // Skip UTF-8 BOM if present
    if content.starts_with(b"\xef\xbb\xbf") {
        pos = 3;
    }

    // Helper to find end of current line
    let find_line_end = |start: usize| -> usize {
        content[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| start + i + 1)
            .unwrap_or(content.len())
    };

    // Helper to check if a line is a comment or blank
    let is_comment_or_blank = |line: &[u8]| -> bool {
        let trimmed = line
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|i| &line[i..])
            .unwrap_or(&[]);
        trimmed.is_empty() || trimmed.starts_with(b"#")
    };

    // Helper to check for encoding declaration (PEP 263)
    // Must be a comment line containing "coding" followed by ":" or "="
    let is_encoding_line = |line: &[u8]| -> bool {
        let trimmed = line
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|i| &line[i..])
            .unwrap_or(&[]);
        if !trimmed.starts_with(b"#") {
            return false;
        }
        let comment = &trimmed[1..];
        // Match PEP 263 pattern: "coding" followed by ":" or "="
        comment
            .windows(7)
            .any(|w| &w[..6] == b"coding" && (w[6] == b':' || w[6] == b'='))
    };

    // Skip shebang (must be first line)
    let pos_after_bom = pos;
    if pos < content.len() {
        let line_end = find_line_end(pos);
        let line = &content[pos..line_end];
        if line.starts_with(b"#!") {
            pos = line_end;
        }
    }

    // Skip encoding declaration if present in the first two physical lines (PEP 263).
    // If we consumed a shebang above, we only need to check the next line.
    // Otherwise check both the current and next line.
    let lines_to_check: usize = if pos > pos_after_bom { 1 } else { 2 };
    for _ in 0..lines_to_check {
        if pos >= content.len() {
            break;
        }
        let line_end = find_line_end(pos);
        let line = &content[pos..line_end];
        if is_encoding_line(line) {
            pos = line_end;
            break;
        }
    }

    // Track minimum insertion point (after BOM/shebang/encoding)
    let mut insertion_point = pos;

    // Skip blank lines and comments before docstring
    while pos < content.len() {
        let line_end = find_line_end(pos);
        let line = &content[pos..line_end];
        if is_comment_or_blank(line) {
            pos = line_end;
            insertion_point = pos;
        } else {
            break;
        }
    }

    // Check for module docstring (triple-quoted string as first statement)
    if pos < content.len() {
        let trimmed_start = content[pos..]
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map(|i| pos + i)
            .unwrap_or(pos);

        let rest = &content[trimmed_start..];
        let quote = if rest.starts_with(b"\"\"\"") {
            Some(b"\"\"\"".as_slice())
        } else if rest.starts_with(b"'''") {
            Some(b"'''".as_slice())
        } else if rest.starts_with(b"r\"\"\"")
            || rest.starts_with(b"R\"\"\"")
            || rest.starts_with(b"u\"\"\"")
            || rest.starts_with(b"U\"\"\"")
        {
            Some(b"\"\"\"".as_slice())
        } else if rest.starts_with(b"r'''")
            || rest.starts_with(b"R'''")
            || rest.starts_with(b"u'''")
            || rest.starts_with(b"U'''")
        {
            Some(b"'''".as_slice())
        } else {
            None
        };

        if let Some(q) = quote {
            // Find the closing triple quote
            let start_offset = if rest.starts_with(b"r")
                || rest.starts_with(b"R")
                || rest.starts_with(b"u")
                || rest.starts_with(b"U")
            {
                4
            } else {
                3
            };
            if let Some(end_idx) = rest[start_offset..]
                .windows(3)
                .position(|w| w == q)
                .map(|i| trimmed_start + start_offset + i + 3)
            {
                // Move past the docstring and any trailing newline
                pos = end_idx;
                if pos < content.len() && content[pos] == b'\n' {
                    pos += 1;
                }
                insertion_point = pos;
            }
        }
    }

    // Now scan for `from __future__` imports
    let mut in_future_import = false;
    let mut paren_depth: usize = 0;

    while pos < content.len() {
        let line_end = find_line_end(pos);
        let line = &content[pos..line_end];

        if in_future_import {
            // Count parentheses in this continuation line
            for &b in line {
                match b {
                    b'(' => paren_depth += 1,
                    b')' => paren_depth = paren_depth.saturating_sub(1),
                    _ => {}
                }
            }
            let ends_with_backslash = !line.is_empty()
                && line
                    .iter()
                    .rposition(|b| !b.is_ascii_whitespace())
                    .map(|i| line[i] == b'\\')
                    .unwrap_or(false);
            if paren_depth == 0 && !ends_with_backslash {
                in_future_import = false;
                insertion_point = line_end;
            }
        } else {
            let trimmed_start = line
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .unwrap_or(0);

            if line[trimmed_start..].starts_with(b"from __future__") {
                // Check if this is a multi-line import
                paren_depth = 0;
                for &b in line {
                    match b {
                        b'(' => paren_depth += 1,
                        b')' => paren_depth = paren_depth.saturating_sub(1),
                        _ => {}
                    }
                }
                let ends_with_backslash = !line.is_empty()
                    && line
                        .iter()
                        .rposition(|b| !b.is_ascii_whitespace())
                        .map(|i| line[i] == b'\\')
                        .unwrap_or(false);
                if paren_depth > 0 || ends_with_backslash {
                    in_future_import = true;
                } else {
                    insertion_point = line_end;
                }
            } else if !is_comment_or_blank(line) {
                // Non-future import statement found, stop scanning
                break;
            }
        }

        pos = line_end;
    }

    insertion_point
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
        const EMPTY: &[u8] = &[];
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

    #[test]
    fn virtual_writer_force_bypasses_excludes() -> Result<()> {
        use std::io::Write as _;

        // Create a temp file to use as a source
        let tmp_dir = TempDir::new()?;
        let source_file = tmp_dir.path().join("artifact.so");
        {
            let mut file = fs_err::File::create(&source_file)?;
            file.write_all(b"test artifact")?;
        }

        // Set up excludes that would match the source file
        let mut excludes = OverrideBuilder::new(tmp_dir.path());
        excludes.add("*.so")?;
        let mut writer = VirtualWriter::new(MockWriter::default(), excludes.build()?);

        // Regular add_file should be excluded by the source path
        writer.add_file("excluded.so", &source_file, true)?;
        assert!(
            writer.tracker.is_empty(),
            "Regular add_file should be excluded"
        );

        // add_file_force should bypass exclusion
        writer.add_file_force("forced.so", &source_file, true)?;
        assert_eq!(
            writer.tracker.len(),
            1,
            "add_file_force should bypass exclusion"
        );

        let files = writer.finish()?;
        assert!(files.contains_key(Path::new("forced.so")));
        assert!(!files.contains_key(Path::new("excluded.so")));

        tmp_dir.close()?;
        Ok(())
    }

    #[test]
    fn test_find_python_insertion_point() {
        use super::find_python_insertion_point;

        // No __future__ imports, no preamble
        assert_eq!(find_python_insertion_point(b"import os\n"), 0);

        // With __future__ import
        let content = b"from __future__ import annotations\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 35); // after the newline

        // Multiple __future__ imports
        let content =
            b"from __future__ import annotations\nfrom __future__ import division\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 67);

        // Docstring then __future__ - insertion after __future__, not after docstring
        let content = b"\"\"\"Docstring.\"\"\"\nfrom __future__ import annotations\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 52);

        // Docstring only (no __future__) - insertion after docstring
        let content = b"\"\"\"Module docstring.\"\"\"\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 24);

        // Empty content
        assert_eq!(find_python_insertion_point(b""), 0);

        // Multi-line parenthesized __future__ import
        let content = b"from __future__ import (\n    annotations,\n)\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 44); // after closing paren line

        // Backslash-continued __future__ import
        let content = b"from __future__ import annotations, \\\n    division\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 51); // after continuation line

        // UTF-8 BOM should be skipped
        let content = b"\xef\xbb\xbfimport os\n";
        assert_eq!(find_python_insertion_point(content), 3); // after BOM

        // UTF-8 BOM with __future__
        let content = b"\xef\xbb\xbffrom __future__ import annotations\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 38); // BOM(3) + future line(35)

        // Shebang line
        let content = b"#!/usr/bin/env python\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 22); // after shebang

        // Shebang + encoding
        let content = b"#!/usr/bin/env python\n# -*- coding: utf-8 -*-\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 46); // after encoding line

        // Shebang + encoding + __future__
        let content =
            b"#!/usr/bin/env python\n# -*- coding: utf-8 -*-\nfrom __future__ import annotations\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 81);

        // Encoding only (no shebang)
        let content = b"# coding: utf-8\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 16);

        // Raw docstring
        let content = b"r\"\"\"Raw docstring.\"\"\"\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 22);

        // Single-quoted docstring
        let content = b"'''Single quoted docstring.'''\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 31);

        // Multi-line docstring
        let content = b"\"\"\"Multi-line\ndocstring.\"\"\"\nimport os\n";
        assert_eq!(find_python_insertion_point(content), 28);
    }

    #[test]
    fn virtual_writer_force_detects_duplicates() -> Result<()> {
        use std::io::Write as _;

        let tmp_dir = TempDir::new()?;
        let source_file = tmp_dir.path().join("artifact.so");
        {
            let mut file = fs_err::File::create(&source_file)?;
            file.write_all(b"test artifact")?;
        }

        let mut writer = VirtualWriter::new(MockWriter::default(), Override::empty());

        // First add should succeed
        writer.add_file_force("target.so", &source_file, true)?;
        assert_eq!(writer.tracker.len(), 1);

        // Second add to same target should fail
        let result = writer.add_file_force("target.so", &source_file, true);
        assert!(result.is_err(), "Duplicate add_file_force should fail");

        tmp_dir.close()?;
        Ok(())
    }
}
