use self::package_name_validations::{cargo_check_name, pypi_check_name};
use crate::ci::GenerateCI;
use crate::{BridgeModel, PyO3};
use anyhow::{Context, Result, bail};
use console::style;
use dialoguer::{Select, theme::ColorfulTheme};
use fs_err as fs;
use minijinja::{Environment, context};
use semver::Version;
use std::path::Path;

/// Mixed Rust/Python project layout
#[derive(Debug, Clone, Copy)]
enum ProjectLayout {
    Mixed { src: bool },
    PureRust,
}

struct ProjectGenerator<'a> {
    env: Environment<'a>,
    project_name: String,
    crate_name: String,
    bindings: String,
    layout: ProjectLayout,
    ci_config: String,
    overwrite: bool,
}

impl ProjectGenerator<'_> {
    fn new(
        project_name: String,
        layout: ProjectLayout,
        bindings: String,
        overwrite: bool,
    ) -> Result<Self> {
        let crate_name = project_name.replace('-', "_");
        let mut env = Environment::new();
        env.set_keep_trailing_newline(true);
        env.add_template(".gitignore", include_str!("templates/.gitignore.j2"))?;
        env.add_template("Cargo.toml", include_str!("templates/Cargo.toml.j2"))?;
        env.add_template(
            "pyproject.toml",
            include_str!("templates/pyproject.toml.j2"),
        )?;
        env.add_template("lib.rs", include_str!("templates/lib.rs.j2"))?;
        env.add_template("main.rs", include_str!("templates/main.rs.j2"))?;
        env.add_template("build.rs", include_str!("templates/build.rs.j2"))?;
        env.add_template("__init__.py", include_str!("templates/__init__.py.j2"))?;
        env.add_template("test_all.py", include_str!("templates/test_all.py.j2"))?;
        env.add_template("example.udl", include_str!("templates/example.udl.j2"))?;

        let bridge_model = match bindings.as_str() {
            "bin" => BridgeModel::Bin(None),
            "cffi" => BridgeModel::Cffi,
            "uniffi" => BridgeModel::UniFfi,
            _ => BridgeModel::PyO3(PyO3 {
                crate_name: bindings.parse()?,
                version: Version::new(0, 23, 1),
                abi3: None,
                metadata: None,
            }),
        };
        let ci_config =
            GenerateCI::default().generate_github(&project_name, &bridge_model, true)?;

        Ok(Self {
            env,
            project_name,
            crate_name,
            bindings,
            layout,
            ci_config,
            overwrite,
        })
    }

    fn generate(&self, project_path: &Path) -> Result<()> {
        fs::create_dir_all(project_path)?;
        self.write_project_file(project_path, ".gitignore")?;
        self.write_project_file(project_path, "pyproject.toml")?;

        // CI configuration
        let gh_action_path = project_path.join(".github").join("workflows");
        fs::create_dir_all(&gh_action_path)?;
        self.write_content(&gh_action_path, "CI.yml", self.ci_config.as_bytes())?;

        let rust_project = match self.layout {
            ProjectLayout::Mixed { src } => {
                let python_dir = if src {
                    project_path.join("src")
                } else {
                    project_path.join("python")
                };
                let python_project = python_dir.join(&self.crate_name);
                fs::create_dir_all(&python_project)?;
                self.write_project_file(&python_project, "__init__.py")?;

                let test_dir = python_dir.join("tests");
                fs::create_dir_all(&test_dir)?;
                self.write_project_file(&test_dir, "test_all.py")?;

                if src {
                    project_path.join("rust")
                } else {
                    project_path.to_path_buf()
                }
            }
            ProjectLayout::PureRust => project_path.to_path_buf(),
        };

        let rust_src = rust_project.join("src");
        fs::create_dir_all(&rust_src)?;
        self.write_project_file(&rust_project, "Cargo.toml")?;
        if self.bindings == "bin" {
            self.write_project_file(&rust_src, "main.rs")?;
        } else {
            self.write_project_file(&rust_src, "lib.rs")?;
            if self.bindings == "uniffi" {
                self.write_project_file(&rust_project, "build.rs")?;
                self.write_project_file(&rust_src, "example.udl")?;
            }
        }

        Ok(())
    }

    fn render_template(&self, tmpl_name: &str) -> Result<String> {
        let version_major: usize = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap();
        let version_minor: usize = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap();
        let tmpl = self.env.get_template(tmpl_name)?;
        let out = tmpl.render(context!(
            name => self.project_name,
            crate_name => self.crate_name,
            bindings => self.bindings,
            mixed_non_src => matches!(self.layout, ProjectLayout::Mixed { src: false }),
            version_major => version_major,
            version_minor => version_minor
        ))?;
        Ok(out)
    }

    fn write_project_file(&self, directory: &Path, file: &str) -> Result<()> {
        let content = self.render_template(file)?;
        self.write_content(directory, file, content.as_bytes())
    }

    fn write_content(&self, directory: &Path, file: &str, content: &[u8]) -> Result<()> {
        let path = directory.join(file);
        if self.overwrite || !path.exists() {
            fs::write(path, content)?;
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> anyhow::Result<String> {
    cargo_check_name(name).context("Invalid Cargo package name")?;
    pypi_check_name(name).context("Invalid PyPI package name")?;
    Ok(name.to_string())
}
/// Options common to `maturin new` and `maturin init`.
#[derive(Debug, clap::Parser)]
pub struct GenerateProjectOptions {
    /// Set the resulting package name, defaults to the directory name
    #[arg(
        long,
        value_parser=validate_name,
    )]
    name: Option<String>,
    /// Use mixed Rust/Python project layout
    #[arg(long)]
    mixed: bool,
    /// Use Python first src layout for mixed Rust/Python project
    #[arg(long)]
    src: bool,
    /// Which kind of bindings to use
    #[arg(
        short,
        long,
        value_parser = ["pyo3", "cffi", "uniffi", "bin"]
    )]
    bindings: Option<String>,
}

/// Generate a new cargo project
pub fn new_project(path: String, options: GenerateProjectOptions) -> Result<()> {
    let project_path = Path::new(&path);
    if project_path.exists() {
        bail!("destination `{}` already exists", project_path.display());
    }
    generate_project(project_path, options, true)?;
    eprintln!(
        "  ✨ {} {} {}",
        style("Done!").bold().green(),
        style("New project created").bold(),
        style(&project_path.display()).underlined()
    );
    Ok(())
}

/// Generate a new cargo project in an existing directory
pub fn init_project(path: Option<String>, options: GenerateProjectOptions) -> Result<()> {
    let project_path = path
        .map(Into::into)
        .map_or_else(std::env::current_dir, Ok)?;
    if project_path.join("pyproject.toml").exists() || project_path.join("Cargo.toml").exists() {
        bail!("`maturin init` cannot be run on existing projects");
    }
    generate_project(&project_path, options, false)?;
    eprintln!(
        "  ✨ {} {} {}",
        style("Done!").bold().green(),
        style("Initialized project").bold(),
        style(&project_path.display()).underlined()
    );
    Ok(())
}

fn generate_project(
    project_path: &Path,
    options: GenerateProjectOptions,
    overwrite: bool,
) -> Result<()> {
    let name = if let Some(name) = options.name {
        name
    } else {
        let file_name = project_path.file_name().with_context(|| {
            format!("Failed to get name from path '{}'", project_path.display())
        })?;
        let temp = file_name
            .to_str()
            .context("Filename isn't valid Unicode")?
            .to_string();

        validate_name(temp.as_str()).map_err(|e| anyhow::anyhow!(e))?
    };
    let bindings_items = if options.mixed {
        vec!["pyo3", "cffi", "uniffi"]
    } else {
        vec!["pyo3", "cffi", "uniffi", "bin"]
    };
    let bindings = if let Some(bindings) = options.bindings {
        bindings
    } else {
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "🤷 {}\n  📖 {}",
                style("Which kind of bindings to use?").bold(),
                style("Documentation: https://maturin.rs/bindings.html").dim()
            ))
            .items(&bindings_items)
            .default(0)
            .interact()?;
        bindings_items[selection].to_string()
    };

    let layout = if options.mixed {
        ProjectLayout::Mixed { src: options.src }
    } else {
        ProjectLayout::PureRust
    };
    let generator = ProjectGenerator::new(name, layout, bindings, overwrite)?;
    generator.generate(project_path)
}

mod package_name_validations {
    // based on: https://github.com/pypi/warehouse/blob/8f79d90a310f0243ab15f52c41de093708a61dfd/warehouse/packaging/models.py#L211C9-L214C10
    pub fn pypi_check_name(name: &str) -> anyhow::Result<()> {
        // The `(?i)` flag was added to make the regex case-insensitive
        let pattern = regex::Regex::new(r"^((?i)[A-Z0-9]|[A-Z0-9][A-Z0-9._-]*[A-Z0-9])$").unwrap();

        if !pattern.is_match(name) {
            anyhow::bail!("The name `{}` is not a valid package name", name)
        }
        Ok(())
    }

    // Based on: https://github.com/rust-lang/cargo/blob/e975158c1b542aa3833fd8584746538c17a6ae55/src/cargo/ops/cargo_new.rs#L169
    pub fn cargo_check_name(name: &str) -> anyhow::Result<()> {
        // Instead of `PackageName::new` which performs these checks in the original cargo code
        validate_package_name(name)?;

        if is_keyword(name) {
            anyhow::bail!(
                "the name `{}` cannot be used as a package name, it is a Rust keyword",
                name,
            );
        }
        if is_conflicting_artifact_name(name) {
            anyhow::bail!(
                "the name `{}` cannot be used as a package name, \
                    it conflicts with cargo's build directory names",
                name,
            );
        }
        if name == "test" {
            anyhow::bail!(
                "the name `test` cannot be used as a package name, \
                it conflicts with Rust's built-in test library",
            );
        }
        if ["core", "std", "alloc", "proc_macro", "proc-macro"].contains(&name) {
            eprintln!(
                "⚠️  Warning: the name `{name}` is part of Rust's standard library\n\
                It is recommended to use a different name to avoid problems.",
            );
        }
        if is_windows_reserved(name) {
            eprintln!(
                "⚠️  Warning: the name `{name}` is a reserved Windows filename\n\
                This package will not work on Windows platforms.",
            );
        }
        if is_non_ascii_name(name) {
            eprintln!(
                "⚠️  Warning: the name `{name}` contains non-ASCII characters\n\
                Non-ASCII crate names are not supported by Rust.",
            );
        }
        let name_in_lowercase = name.to_lowercase();
        if name != name_in_lowercase {
            eprintln!(
                "⚠️  Warning: the name `{name}` is not snake_case or kebab-case which is recommended for package names, consider `{name_in_lowercase}`"
            );
        }

        Ok(())
    }

    // Based on: https://github.com/rust-lang/cargo/blob/7b7af3077bff8d60b7f124189bc9de227d3063a9/crates/cargo-util-schemas/src/restricted_names.rs#L42
    fn validate_package_name(name: &str) -> anyhow::Result<()> {
        if name.is_empty() {
            anyhow::bail!("Package names cannot be empty");
        }

        let mut chars = name.chars();
        if let Some(ch) = chars.next() {
            if ch.is_ascii_digit() {
                // A specific error for a potentially common case.
                anyhow::bail!("Package names cannot start with a digit");
            }
            if !(unicode_xid::UnicodeXID::is_xid_start(ch) || ch == '_') {
                anyhow::bail!(
                    "the first character must be a Unicode XID start character (most letters or `_`)"
                );
            }
        }
        for ch in chars {
            if !(unicode_xid::UnicodeXID::is_xid_continue(ch) || ch == '-') {
                anyhow::bail!(
                    "characters must be Unicode XID characters (numbers, `-`, `_`, or most letters)"
                );
            }
        }
        Ok(())
    }

    // The following functions are based on https://github.com/rust-lang/cargo/blob/e975158c1b542aa3833fd8584746538c17a6ae55/src/cargo/util/restricted_names.rs

    /// Returns `true` if the name contains non-ASCII characters.
    pub fn is_non_ascii_name(name: &str) -> bool {
        name.chars().any(|ch| ch > '\x7f')
    }

    /// A Rust keyword.
    pub fn is_keyword(name: &str) -> bool {
        // See https://doc.rust-lang.org/reference/keywords.html
        [
            "Self", "abstract", "as", "async", "await", "become", "box", "break", "const",
            "continue", "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn",
            "for", "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut",
            "override", "priv", "pub", "ref", "return", "self", "static", "struct", "super",
            "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual",
            "where", "while", "yield",
        ]
        .contains(&name)
    }

    /// These names cannot be used on Windows, even with an extension.
    pub fn is_windows_reserved(name: &str) -> bool {
        [
            "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
            "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
        ]
        .contains(&name.to_ascii_lowercase().as_str())
    }

    /// An artifact with this name will conflict with one of Cargo's build directories.
    pub fn is_conflicting_artifact_name(name: &str) -> bool {
        ["deps", "examples", "build", "incremental"].contains(&name)
    }
}
