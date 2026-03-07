/// A minimal YAML writer that tracks indentation level.
///
/// Each `line()` call prepends `level * 2` spaces. Use `indent()`/`dedent()`
/// to manage nesting. This removes hardcoded indentation from the YAML
/// generation code.
pub(super) struct Yaml<'a> {
    out: &'a mut String,
    level: usize,
}

impl<'a> Yaml<'a> {
    pub(super) fn new(out: &'a mut String, level: usize) -> Self {
        Self { out, level }
    }

    /// Write a single line at the current indentation level.
    pub(super) fn line(&mut self, s: impl AsRef<str>) -> &mut Self {
        for _ in 0..self.level {
            self.out.push_str("  ");
        }
        self.out.push_str(s.as_ref());
        self.out.push('\n');
        self
    }

    pub(super) fn indent_by(&mut self, n: usize) -> &mut Self {
        self.level += n;
        self
    }

    pub(super) fn indent(&mut self) -> &mut Self {
        self.indent_by(1)
    }

    pub(super) fn dedent_by(&mut self, n: usize) -> &mut Self {
        debug_assert!(self.level >= n);
        self.level -= n;
        self
    }

    pub(super) fn dedent(&mut self) -> &mut Self {
        self.dedent_by(1)
    }
}
