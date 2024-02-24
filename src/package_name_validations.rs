// Based on: https://github.com/rust-lang/cargo/blob/e975158c1b542aa3833fd8584746538c17a6ae55/src/cargo/ops/cargo_new.rs#L169
pub fn cargo_check_name(name: &str) -> anyhow::Result<()> {
    // Instead of `PackageName::new`, which performs these checks
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
        //  TODO: shell.warn?
        println!(
            "the name `{}` is part of Rust's standard library\n\
            It is recommended to use a different name to avoid problems.",
            name,
        );
    }
    if is_windows_reserved(name) {
        // TODO: ????
        if cfg!(windows) {
            anyhow::bail!(
                "cannot use name `{}`, it is a reserved Windows filename",
                name,
            );
        } else {
            //  TODO: shell.warn?
            println!(
                "the name `{}` is a reserved Windows filename\n\
                This package will not work on Windows platforms.",
                name
            );
        }
    }
    if is_non_ascii_name(name) {
        //  TODO: shell.warn?
        println!(
            "the name `{}` contains non-ASCII characters\n\
            Non-ASCII crate names are not supported by Rust.",
            name
        );
    }
    let name_in_lowercase = name.to_lowercase();
    if name != name_in_lowercase {
        //  TODO: shell.warn?
        println!(
            "the name `{name}` is not snake_case or kebab-case which is recommended for package names, consider `{name_in_lowercase}`"
        );
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
        "Self", "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
        "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "if",
        "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv",
        "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "try",
        "type", "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
    ]
    .contains(&name)
}

/// These names cannot be used on Windows, even with an extension.
pub fn is_windows_reserved(name: &str) -> bool {
    [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ]
    .contains(&name.to_ascii_lowercase().as_str())
}

/// An artifact with this name will conflict with one of Cargo's build directories.
pub fn is_conflicting_artifact_name(name: &str) -> bool {
    ["deps", "examples", "build", "incremental"].contains(&name)
}

// Based on: https://github.com/rust-lang/cargo/blob/7b7af3077bff8d60b7f124189bc9de227d3063a9/crates/cargo-util-schemas/src/restricted_names.rs#L42
fn validate_package_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("Package names cannot be empty");
    }

    let mut chars = name.chars();
    if let Some(ch) = chars.next() {
        if ch.is_digit(10) {
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
