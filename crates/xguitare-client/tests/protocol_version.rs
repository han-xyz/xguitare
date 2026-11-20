use xguitare_client::newer_protocol_warning;
use xguitare_protocol::Value;

#[test]
fn only_a_strictly_higher_integer_protocol_version_gets_a_warning() {
    let warning = |version: &str| newer_protocol_warning(&version.parse::<Value>().unwrap());
    let text = |daemon: &str| {
        Some(format!(
            "the Daemon speaks protocol {daemon}, this xguitare knows 1; update xguitare"
        ))
    };
    assert_eq!(warning(r#"{"ok":true}"#), None);
    assert_eq!(warning(r#"{"protocol_version":null}"#), None);
    assert_eq!(warning(r#"{"protocol_version":"2"}"#), None);
    assert_eq!(warning(r#"{"protocol_version":-3}"#), None);
    assert_eq!(warning(r#"{"protocol_version":2.5}"#), None);
    assert_eq!(warning(r#"{"protocol_version":1e30}"#), None);
    assert_eq!(warning(r#"{"protocol_version":0}"#), None);
    assert_eq!(warning(r#"{"protocol_version":1}"#), None);
    assert_eq!(warning(r#"{"protocol_version":2}"#), text("2"));
    assert_eq!(
        warning(r#"{"protocol_version":18446744073709551615}"#),
        text("18446744073709551615")
    );
}
