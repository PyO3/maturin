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
}
