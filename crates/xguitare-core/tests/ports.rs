use xguitare_core::Ports;
use xguitare_core::engine::choose_ports;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

fn ports(input: &str, output_left: &str, output_right: &str) -> Ports {
    Ports {
        input: input.to_string(),
        output_left: output_left.to_string(),
        output_right: output_right.to_string(),
    }
}

fn capture() -> Vec<String> {
    names(&["system:capture_1", "system:capture_2"])
}

fn playback() -> Vec<String> {
    names(&["system:playback_1", "system:playback_2"])
}

#[test]
fn choose_ports_defaults() {
    let choice = choose_ports(
        &capture(),
        &playback(),
        &ports("system:capture_1", "system:playback_1", "system:playback_2"),
    );
    assert_eq!(choice.input.as_deref(), Some("system:capture_1"));
    assert_eq!(choice.output_left.as_deref(), Some("system:playback_1"));
    assert_eq!(choice.output_right.as_deref(), Some("system:playback_2"));
}

#[test]
fn choose_ports_config_override() {
    let choice = choose_ports(
        &capture(),
        &playback(),
        &ports("system:capture_2", "system:playback_1", "system:playback_2"),
    );
    assert_eq!(choice.input.as_deref(), Some("system:capture_2"));
}

#[test]
fn choose_ports_missing_is_none() {
    let choice = choose_ports(
        &capture(),
        &playback(),
        &ports("system:capture_9", "system:playback_1", "system:playback_7"),
    );
    assert_eq!(choice.input, None);
    assert_eq!(choice.output_left.as_deref(), Some("system:playback_1"));
    assert_eq!(choice.output_right, None);
}

#[test]
fn choose_ports_empty_means_none() {
    let choice = choose_ports(&capture(), &playback(), &ports("", "", ""));
    assert_eq!(choice.input, None);
    assert_eq!(choice.output_left, None);
    assert_eq!(choice.output_right, None);
}
