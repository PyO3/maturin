use anyhow::{bail, Result};
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
}

impl<'a> ProjectGenerator<'a> {
    fn new(project_name: String, mixed: bool, bindings: String) -> Result<Self> {
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
        Ok(Self {
            env,
            project_name,
            crate_name,
            bindings,
            mixed,
        })
    }

    fn generate(&self) -> Result<()> {
        let project_path = Path::new(&self.project_name);
        if project_path.exists() {
            bail!("destination `{}` already exists", project_path.display());
        }

        let src_path = project_path.join("src");
        fs::create_dir_all(&src_path)?;

        let gitignore = self.render_template(".gitignore")?;
        fs::write(project_path.join(".gitignore"), gitignore)?;

        let cargo_toml = self.render_template("Cargo.toml")?;
        fs::write(project_path.join("Cargo.toml"), cargo_toml)?;

        let pyproject_toml = self.render_template("pyproject.toml")?;
        fs::write(project_path.join("pyproject.toml"), pyproject_toml)?;

        if self.bindings == "bin" {
            let main_rs = self.render_template("main.rs")?;
            fs::write(src_path.join("main.rs"), main_rs)?;
        } else {
            let lib_rs = self.render_template("lib.rs")?;
            fs::write(src_path.join("lib.rs"), lib_rs)?;
        }

        if self.mixed {
            let py_path = project_path.join(&self.crate_name);
            fs::create_dir_all(&py_path)?;
            let init_py = self.render_template("__init__.py")?;
            fs::write(py_path.join("__init__.py"), init_py)?;
        }

        println!(
            "  ✨ {} {} {}",
            style("Done!").bold().green(),
            style("New project created").bold(),
            style(&project_path.display()).underlined()
        );
        Ok(())
    }

    fn render_template(&self, tmpl_name: &str) -> Result<String> {
        let tmpl = self.env.get_template(tmpl_name)?;
        let out =
            tmpl.render(context!(name => self.project_name, crate_name => self.crate_name, bindings => self.bindings))?;
        Ok(out)
    }
}

/// Generate a new cargo project
pub fn new_project(name: String, mixed: bool, bindings: Option<String>) -> Result<()> {
    let bindings_items = if mixed {
        vec!["pyo3", "rust-cpython", "cffi"]
    } else {
        vec!["pyo3", "rust-cpython", "cffi", "bin"]
    };
    let bindings = if let Some(bindings) = bindings {
        bindings
    } else {
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "🤷 {}",
                style("What kind of bindings to use?").bold()
            ))
            .items(&bindings_items)
            .default(0)
            .interact()?;
        bindings_items[selection].to_string()
    };

    let generator = ProjectGenerator::new(name, mixed, bindings)?;
    generator.generate()
}
