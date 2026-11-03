use xguitare_core::Ports;
use xguitare_core::engine::{AvailablePort, choose_ports};

fn available(list: &[&str], is_physical: bool) -> Vec<AvailablePort> {
    list.iter()
        .map(|name| AvailablePort {
            name: name.to_string(),
            is_physical,
        })
        .collect()
}

fn names(list: &[&str]) -> Vec<AvailablePort> {
    available(list, true)
}

fn ports(input: &str, output_left: &str, output_right: &str) -> Ports {
    Ports {
        input: input.to_string(),
        output_left: output_left.to_string(),
        output_right: output_right.to_string(),
    }
}

fn capture() -> Vec<AvailablePort> {
    names(&["card:capture_FR", "card:capture_FL"])
}

fn playback() -> Vec<AvailablePort> {
    names(&["card:playback_FR", "card:playback_FL"])
}

#[test]
fn choose_ports_auto_single_device() {
    let choice = choose_ports(&capture(), &playback(), &ports("auto", "auto", "auto"));
    assert_eq!(choice.input.as_deref(), Some("card:capture_FL"));
    assert_eq!(choice.output_left.as_deref(), Some("card:playback_FL"));
    assert_eq!(choice.output_right.as_deref(), Some("card:playback_FR"));
    assert!(choice.warnings.is_empty(), "{:?}", choice.warnings);
}

#[test]
fn choose_ports_auto_several_devices_picks_none() {
    let choice = choose_ports(
        &names(&["mic:capture_MONO", "card:capture_FL"]),
        &names(&["hdmi:playback_FL", "card:playback_FL"]),
        &ports("auto", "auto", "auto"),
    );
    assert_eq!(choice.input, None);
    assert_eq!(choice.output_left, None);
    assert_eq!(choice.output_right, None);
    assert_eq!(
        choice.warnings,
        [
            "several capture devices; pick one in config ports.input: card:capture_FL, mic:capture_MONO",
            "several playback devices; pick one in config ports.output_left, ports.output_right: card:playback_FL, hdmi:playback_FL",
        ]
    );
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
    assert_eq!(choice.warnings.len(), 1, "{:?}", choice.warnings);
}

#[test]
fn choose_ports_pattern_wildcard() {
    let available = names(&[
        "Razer Mono:capture_MONO",
        "Split Scarlett Input 1:capture_FL",
        "Scarlett Input 1:capture_FR",
        "Scarlett Input 1:capture_FL",
    ]);
    let choice = choose_ports(
        &available,
        &playback(),
        &ports("Scarlett*capture_FL", "", ""),
    );
    assert_eq!(choice.input.as_deref(), Some("Scarlett Input 1:capture_FL"));
    let reversed = choose_ports(
        &available,
        &playback(),
        &ports("capture_FL*Scarlett", "", ""),
    );
    assert_eq!(reversed.input, None);
}

#[test]
fn choose_ports_pattern_exact() {
    let exact = choose_ports(&capture(), &playback(), &ports("card:capture_FR", "", ""));
    assert_eq!(exact.input.as_deref(), Some("card:capture_FR"));
    let partial = choose_ports(&capture(), &playback(), &ports("card:capture", "", ""));
    assert_eq!(partial.input, None);
}

#[test]
fn choose_ports_pattern_no_match() {
    let choice = choose_ports(&capture(), &playback(), &ports("Focusrite*", "", ""));
    assert_eq!(choice.input, None);
    assert_eq!(
        choice.warnings,
        ["port 'Focusrite*' not found, not connecting"]
    );
}

#[test]
fn choose_ports_empty_means_none() {
    let choice = choose_ports(&capture(), &playback(), &ports("", "", ""));
    assert_eq!(choice.input, None);
    assert_eq!(choice.output_left, None);
    assert_eq!(choice.output_right, None);
    assert!(choice.warnings.is_empty(), "{:?}", choice.warnings);
}

#[test]
fn choose_ports_pattern_matches_non_physical() {
    let mut capture = names(&["Scarlett 0:capture_AUX0"]);
    capture.extend(available(&["Scarlett Input 1:capture_FL"], false));
    let choice = choose_ports(&capture, &playback(), &ports("Scarlett*capture_FL", "", ""));
    assert_eq!(choice.input.as_deref(), Some("Scarlett Input 1:capture_FL"));
    let auto = choose_ports(&capture, &playback(), &ports("auto", "", ""));
    assert_eq!(auto.input.as_deref(), Some("Scarlett 0:capture_AUX0"));
}
