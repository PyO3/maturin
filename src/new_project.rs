use crate::ci::GenerateCI;
use crate::BridgeModel;
use anyhow::{bail, Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};
use fs_err as fs;
use minijinja::{context, Environment};
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

impl<'a> ProjectGenerator<'a> {
    fn new(
        project_name: String,
        layout: ProjectLayout,
        bindings: String,
        overwrite: bool,
    ) -> Result<Self> {
        let crate_name = project_name.replace('-', "_");
        let mut env = Environment::new();
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
        env.add_template("example.udl", include_str!("templates/example.udl.j2"))?;

        let bridge_model = match bindings.as_str() {
            "bin" => BridgeModel::Bin(None),
            "cffi" => BridgeModel::Cffi,
            "uniffi" => BridgeModel::UniFfi,
            _ => BridgeModel::Bindings(bindings.clone(), 7),
        };
        let ci_config = GenerateCI::default().generate_github(&project_name, &bridge_model)?;

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

/// Options common to `maturin new` and `maturin init`.
#[derive(Debug, clap::Parser)]
pub struct GenerateProjectOptions {
    /// Set the resulting package name, defaults to the directory name
    #[arg(long)]
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
        value_parser = ["pyo3", "rust-cpython", "cffi", "uniffi", "bin"]
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
    println!(
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
    println!(
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
        file_name
            .to_str()
            .context("Filename isn't valid Unicode")?
            .to_string()
    };
    let bindings_items = if options.mixed {
        vec!["pyo3", "rust-cpython", "cffi", "uniffi"]
    } else {
        vec!["pyo3", "rust-cpython", "cffi", "uniffi", "bin"]
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
