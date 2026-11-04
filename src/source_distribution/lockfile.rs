//! Version-aware loading of Cargo lockfiles.

use anyhow::{Result, bail};
use cargo_lock::Lockfile;

/// Removing workspace members may drop packages, but must not change retained
/// registry or Git pins. SourceId equality alone is not sufficient: it treats
/// different Git references as equal when they point at the same commit.
pub(super) fn validate_pins(original: &Lockfile, updated: &Lockfile) -> Result<()> {
    for package in &updated.packages {
        let Some(source) = &package.source else {
            continue;
        };
        if !original.packages.iter().any(|locked| {
            locked.name == package.name
                && locked.version == package.version
                && locked.checksum == package.checksum
                && locked.source.as_ref().is_some_and(|locked_source| {
                    locked_source.url() == source.url()
                        && locked_source.kind() == source.kind()
                        && locked_source.precise() == source.precise()
                })
        }) {
            bail!(
                "Updating the source distribution lockfile would change the locked dependency `{} {}`",
                package.name,
                package.version
            );
        }
    }
    Ok(())
}

/// Load legacy lockfiles without interpreting literal Git reference characters
/// as URL escapes. cargo-lock expects encoded source URLs in every format.
pub(super) fn parse(contents: &str) -> Result<Lockfile> {
    let mut lockfile: toml::Value = toml::from_str(contents)?;
    if lockfile.get("version").and_then(toml::Value::as_integer) >= Some(4) {
        return Ok(contents.parse()?);
    }

    fn package(package: &mut toml::Value) {
        if let Some(source) = package.get_mut("source")
            && let Some(value) = source.as_str()
        {
            *source = git_source(value).into();
        }
        if let Some(dependencies) = package
            .get_mut("dependencies")
            .and_then(toml::Value::as_array_mut)
        {
            for dependency in dependencies {
                if let Some(value) = dependency.as_str() {
                    *dependency = package_id(value).into();
                }
            }
        }
        if let Some(replace) = package.get_mut("replace")
            && let Some(value) = replace.as_str()
        {
            *replace = package_id(value).into();
        }
    }

    if let Some(packages) = lockfile
        .get_mut("package")
        .and_then(toml::Value::as_array_mut)
    {
        packages.iter_mut().for_each(package);
    }
    if let Some(root) = lockfile.get_mut("root") {
        package(root);
    }
    if let Some(patches) = lockfile
        .get_mut("patch")
        .and_then(|patch| patch.get_mut("unused"))
        .and_then(toml::Value::as_array_mut)
    {
        patches.iter_mut().for_each(package);
    }
    if let Some(metadata) = lockfile
        .get_mut("metadata")
        .and_then(toml::Value::as_table_mut)
    {
        *metadata = std::mem::take(metadata)
            .into_iter()
            .map(|(key, value)| {
                let key = if key.starts_with("checksum ") {
                    package_id(&key)
                } else {
                    key
                };
                (key, value)
            })
            .collect();
    }
    Ok(toml::to_string(&lockfile)?.parse()?)
}

/// Encode the source in a fully qualified dependency or legacy checksum key.
fn package_id(value: &str) -> String {
    if let Some((package, source)) = value.split_once(" (git+")
        && let Some(source) = source.strip_suffix(')')
    {
        return format!("{package} ({})", git_source(&format!("git+{source}")));
    }
    value.to_owned()
}

/// Only Git reference values changed encoding in v4. In earlier formats the
/// whole value is literal, including `+`, `%`, `&`, and `=`.
fn git_source(source: &str) -> String {
    if source.starts_with("git+")
        && let Some((url, query)) = source.split_once('?')
    {
        let (reference, fragment) = query.rsplit_once('#').unwrap_or((query, ""));
        if let Some((key @ ("branch" | "tag" | "rev" | "ref"), value)) = reference.split_once('=') {
            let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
            let fragment = if fragment.is_empty() {
                String::new()
            } else {
                format!("#{fragment}")
            };
            return format!("{url}?{key}={encoded}{fragment}");
        }
    }
    source.to_owned()
}

#[cfg(test)]
mod tests {
    use super::{parse, validate_pins};

    #[rstest::rstest]
    #[case::branch("?branch=release", "?branch=master")]
    #[case::tag("?tag=release", "?tag=other")]
    #[case::reference_kind("?branch=release", "?tag=release")]
    #[case::revision_reference("?rev=release", "?rev=other")]
    #[case::default_branch("", "?branch=master")]
    fn rejects_changed_git_references(#[case] original: &str, #[case] updated: &str) {
        let lockfile = |reference| {
            parse(&format!(
                "version = 4\n[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\nsource = \"git+https://example.com/dep{reference}#0123456789abcdef0123456789abcdef01234567\"\n"
            ))
            .unwrap()
        };
        assert!(validate_pins(&lockfile(original), &lockfile(updated)).is_err());
    }

    #[rstest::rstest]
    #[case::changed_revision(Some("1123456789abcdef0123456789abcdef01234567"))]
    #[case::missing_revision(None)]
    fn rejects_changed_git_revision(#[case] revision: Option<&str>) {
        let original = parse(
            "version = 4\n[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\nsource = \"git+https://example.com/dep?branch=release#0123456789abcdef0123456789abcdef01234567\"\n",
        )
        .unwrap();
        let mut updated = original.clone();
        let package = &mut updated.packages[0];
        package.source = Some(
            package
                .source
                .as_ref()
                .unwrap()
                .with_precise(revision.map(str::to_owned)),
        );
        assert!(validate_pins(&original, &updated).is_err());
    }

    #[rstest::rstest]
    #[case::name("name")]
    #[case::version("version")]
    #[case::registry("registry")]
    #[case::checksum("checksum")]
    fn rejects_changed_registry_pins(#[case] field: &str) {
        let original = parse(&format!(
            "version = 4\n[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\nsource = \"registry+https://example.com/index\"\nchecksum = \"{}\"\n",
            "0".repeat(64)
        ))
        .unwrap();
        let mut updated = original.clone();
        let package = &mut updated.packages[0];
        match field {
            "name" => package.name = "other".parse().unwrap(),
            "version" => package.version = "0.2.0".parse().unwrap(),
            "registry" => {
                package.source = Some(
                    cargo_lock::SourceId::from_url("registry+https://example.com/other-index")
                        .unwrap(),
                );
            }
            "checksum" => package.checksum = Some("1".repeat(64).parse().unwrap()),
            _ => unreachable!(),
        }
        assert!(validate_pins(&original, &updated).is_err());
    }

    #[test]
    fn allows_pruning_unused_packages_and_changing_lockfile_format() {
        let original = parse(
            "version = 3\n[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\nsource = \"git+https://example.com/dep?branch=feature+sdist#0123456789abcdef0123456789abcdef01234567\"\n\n[[package]]\nname = \"unused\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let mut updated = original.clone();
        updated
            .packages
            .retain(|package| package.name.as_str() != "unused");
        updated.version = cargo_lock::ResolveVersion::V4;
        let updated = parse(&updated.to_string()).unwrap();
        validate_pins(&original, &updated).unwrap();
    }

    #[rstest::rstest]
    #[case("feature/sdist", "feature%2Fsdist")]
    #[case("feature+sdist", "feature%2Bsdist")]
    #[case("feature%2Fsdist", "feature%252Fsdist")]
    #[case("feature&sdist=next", "feature%26sdist%3Dnext")]
    #[case("master", "master")]
    fn legacy_git_references_round_trip(
        #[case] reference: &str,
        #[case] encoded: &str,
        #[values("branch", "tag", "rev")] kind: &str,
    ) {
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let source = format!("git+https://example.com/dep?{kind}={reference}#{revision}");
        let contents = format!(
            "version = 3\n[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\nsource = \"{source}\"\n"
        );
        let mut lockfile = parse(&contents).unwrap();
        lockfile.version = cargo_lock::ResolveVersion::V4;
        let v4 = lockfile.to_string();
        let v4_toml: toml::Value = toml::from_str(&v4).unwrap();
        assert_eq!(
            v4_toml["package"][0]["source"].as_str().unwrap(),
            format!("git+https://example.com/dep?{kind}={encoded}#{revision}")
        );

        let mut lockfile = parse(&v4).unwrap();
        lockfile.version = cargo_lock::ResolveVersion::V3;
        let v3: toml::Value = toml::from_str(&lockfile.to_string()).unwrap();
        assert_eq!(v3["package"][0]["source"].as_str().unwrap(), source);
    }

    #[test]
    fn legacy_checksum_and_unused_patch_sources() {
        let source = "git+https://example.com/dep?branch=feature+sdist#0123456789abcdef0123456789abcdef01234567";
        let checksum = "0".repeat(64);
        let contents = format!(
            "[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\nsource = \"{source}\"\n\n[metadata]\n\"checksum dep 0.1.0 ({source})\" = \"{checksum}\"\n\n[[patch.unused]]\nname = \"unused\"\nversion = \"0.1.0\"\nsource = \"{source}\"\n"
        );
        let lockfile = parse(&contents).unwrap();
        assert_eq!(lockfile.version, cargo_lock::ResolveVersion::V1);
        assert_eq!(
            lockfile.packages[0].checksum.as_ref().unwrap().to_string(),
            checksum
        );
        assert_eq!(
            lockfile.packages[0].source.as_ref().unwrap().to_string(),
            source
        );
        assert_eq!(
            lockfile.patch.unused[0]
                .source
                .as_ref()
                .unwrap()
                .to_string(),
            source
        );
    }
}
