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
    names(&["card:capture_FL", "card:capture_FR"])
}

fn playback() -> Vec<String> {
    names(&["card:playback_FL", "card:playback_FR"])
}

#[test]
fn choose_ports_defaults() {
    let choice = choose_ports(&capture(), &playback(), &ports("auto", "auto", "auto"));
    assert_eq!(choice.input.as_deref(), Some("card:capture_FL"));
    assert_eq!(choice.output_left.as_deref(), Some("card:playback_FL"));
    assert_eq!(choice.output_right.as_deref(), Some("card:playback_FR"));
}

#[test]
fn choose_ports_config_override() {
    let choice = choose_ports(
        &capture(),
        &playback(),
        &ports("other:out", "card:playback_FR", "auto"),
    );
    assert_eq!(choice.input.as_deref(), Some("other:out"));
    assert_eq!(choice.output_left.as_deref(), Some("card:playback_FR"));
    assert_eq!(choice.output_right.as_deref(), Some("card:playback_FR"));
}

#[test]
fn choose_ports_missing_is_none() {
    let choice = choose_ports(&[], &[], &ports("auto", "auto", "auto"));
    assert_eq!(choice.input, None);
    assert_eq!(choice.output_left, None);
    assert_eq!(choice.output_right, None);
}

#[test]
fn choose_ports_empty_means_none() {
    let choice = choose_ports(&capture(), &playback(), &ports("", "", ""));
    assert_eq!(choice.input, None);
    assert_eq!(choice.output_left, None);
    assert_eq!(choice.output_right, None);
}

#[test]
fn choose_ports_auto_picks_first_physical() {
    let choice = choose_ports(
        &names(&["a:capture_1", "b:capture_1"]),
        &playback(),
        &ports("auto", "auto", ""),
    );
    assert_eq!(choice.input.as_deref(), Some("a:capture_1"));
    assert_eq!(choice.output_left.as_deref(), Some("card:playback_FL"));
    assert_eq!(choice.output_right, None);
}

#[test]
fn choose_ports_auto_single_playback_warns() {
    let choice = choose_ports(
        &capture(),
        &names(&["card:playback_MONO"]),
        &ports("auto", "auto", "auto"),
    );
    assert_eq!(choice.output_left.as_deref(), Some("card:playback_MONO"));
    assert_eq!(choice.output_right, None);
}
