#[test]
fn cli_tests() {
    let t = trycmd::TestCases::new();
    t.default_bin_name("maturin");
    t.case("tests/cmd/*.toml");

    #[cfg(not(feature = "upload"))]
    {
        t.skip("tests/cmd/upload.toml");
        t.skip("tests/cmd/publish.toml");
    }

    #[cfg(not(feature = "zig"))]
    {
        t.skip("tests/cmd/build.toml");
    }

    #[cfg(not(feature = "scaffolding"))]
    {
        t.skip("tests/cmd/new.toml");
        t.skip("tests/cmd/init.toml");
        t.skip("tests/cmd/generate-ci.toml");
    }

    #[cfg(not(all(feature = "upload", feature = "zig", feature = "scaffolding")))]
    {
        t.skip("tests/cmd/maturin.toml");
    }
}
