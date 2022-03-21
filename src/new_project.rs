use anyhow::{bail, Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};
use fs_err as fs;
use minijinja::{context, Environment};
use std::path::Path;

struct ProjectGenerator<'a> {
    env: Environment<'a>,
    project_name: String,
    crate_name: String,
    bindings: String,
    mixed: bool,
    overwrite: bool,
}

impl<'a> ProjectGenerator<'a> {
    fn new(project_name: String, mixed: bool, bindings: String, overwrite: bool) -> Result<Self> {
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
        env.add_template("__init__.py", include_str!("templates/__init__.py.j2"))?;
        env.add_template("CI.yml", include_str!("templates/CI.yml.j2"))?;
        Ok(Self {
            env,
            project_name,
            crate_name,
            bindings,
            mixed,
            overwrite,
        })
    }

    fn generate(&self, project_path: &Path) -> Result<()> {
        let src_path = project_path.join("src");
        fs::create_dir_all(&src_path)?;

        self.write_project_file(project_path, ".gitignore")?;
        self.write_project_file(project_path, "Cargo.toml")?;
        self.write_project_file(project_path, "pyproject.toml")?;

        if self.bindings == "bin" {
            self.write_project_file(&src_path, "main.rs")?;
        } else {
            self.write_project_file(&src_path, "lib.rs")?;
        }

        let gh_action_path = project_path.join(".github").join("workflows");
        fs::create_dir_all(&gh_action_path)?;
        self.write_project_file(&gh_action_path, "CI.yml")?;

        if self.mixed {
            let python_dir = project_path.join("python");
            let py_path = python_dir.join(&self.crate_name);
            fs::create_dir_all(&py_path)?;
            self.write_project_file(&py_path, "__init__.py")?;
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
            mixed => self.mixed,
            version_major => version_major,
            version_minor => version_minor
        ))?;
        Ok(out)
    }

    fn write_project_file(&self, directory: &Path, file: &str) -> Result<()> {
        let path = directory.join(file);
        if self.overwrite || !path.exists() {
            fs::write(path, self.render_template(file)?)?;
        }
        Ok(())
    }
}

/// Options common to `maturin new` and `maturin init`.
#[derive(Debug, clap::Parser)]
pub struct GenerateProjectOptions {
    /// Set the resulting package name, defaults to the directory name
    #[clap(long)]
    name: Option<String>,
    /// Use mixed Rust/Python project layout
    #[clap(long)]
    mixed: bool,
    /// Which kind of bindings to use
    #[clap(short, long, possible_values = &["pyo3", "rust-cpython", "cffi", "bin"])]
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
        vec!["pyo3", "rust-cpython", "cffi"]
    } else {
        vec!["pyo3", "rust-cpython", "cffi", "bin"]
    };
    let bindings = if let Some(bindings) = options.bindings {
        bindings
    } else {
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "🤷 {}",
                style("Which kind of bindings to use?").bold()
            ))
            .items(&bindings_items)
            .default(0)
            .interact()?;
        bindings_items[selection].to_string()
    };

    let generator = ProjectGenerator::new(name, options.mixed, bindings, overwrite)?;
    generator.generate(project_path)
}
