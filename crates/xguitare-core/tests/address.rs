use xguitare_core::{ParamAddress, Preset, resolve};

fn parse_error(input: &str) -> String {
    ParamAddress::parse(input).unwrap_err().to_string()
}

fn two_gains() -> Preset {
    Preset::parse(
        "name = \"X\"\n[[module]]\ntype = \"gain\"\nid = \"a\"\n[[module]]\ntype = \"gain\"\nid = \"b\"\n",
    )
    .unwrap()
}

fn resolve_error(input: &str) -> String {
    resolve(&two_gains(), &ParamAddress::parse(input).unwrap())
        .unwrap_err()
        .to_string()
}

#[test]
fn parse_ok() {
    let addr = ParamAddress::parse("preamp.gain").unwrap();
    assert_eq!(
        (addr.module.as_str(), addr.param.as_str()),
        ("preamp", "gain")
    );
}

#[test]
fn parse_missing_dot() {
    assert_eq!(
        parse_error("preamp"),
        "expected <module>.<param>, got 'preamp'"
    );
}

#[test]
fn parse_two_dots() {
    assert_eq!(
        parse_error("a.b.c"),
        "expected <module>.<param>, got 'a.b.c'"
    );
}

#[test]
fn parse_empty_side() {
    assert_eq!(
        parse_error(".gain"),
        "expected <module>.<param>, got '.gain'"
    );
    assert_eq!(
        parse_error("preamp."),
        "expected <module>.<param>, got 'preamp.'"
    );
}

#[test]
fn resolve_ok_indices() {
    let addr = ParamAddress::parse("b.level").unwrap();
    assert_eq!(resolve(&two_gains(), &addr).unwrap(), (1, 0));
}

#[test]
fn resolve_unknown_module() {
    assert_eq!(resolve_error("x.level"), "no module 'x'");
}

#[test]
fn resolve_unknown_param() {
    assert_eq!(
        resolve_error("a.nope"),
        "module 'a' has no parameter 'nope'"
    );
}
