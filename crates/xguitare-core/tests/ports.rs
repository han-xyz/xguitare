use xguitare_core::Ports;
use xguitare_core::ports::{
    AvailablePort, ConnectPlan, capture_track_inputs, choose_ports, connect_plan, foreign_ports,
};

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
    names(&["card:capture_FL", "card:capture_FR"])
}

fn playback() -> Vec<AvailablePort> {
    names(&["card:playback_FL", "card:playback_FR"])
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

#[test]
fn choose_ports_pattern_is_anchored() {
    let capture = names(&["Scarlett:capture_10", "Split Scarlett Input 1:capture_FL"]);
    let choice = choose_ports(&capture, &playback(), &ports("Scarlett*capture_1", "", ""));
    assert_eq!(choice.input, None);
    let choice = choose_ports(&capture, &playback(), &ports("Scarlett*capture_FL", "", ""));
    assert_eq!(choice.input, None);
    let choice = choose_ports(&capture, &playback(), &ports("*capture_FL", "", ""));
    assert_eq!(
        choice.input.as_deref(),
        Some("Split Scarlett Input 1:capture_FL")
    );
}

#[test]
fn choose_ports_auto_uses_jack_order() {
    let surround = names(&[
        "card:playback_FL",
        "card:playback_FR",
        "card:playback_RL",
        "card:playback_RR",
        "card:playback_FC",
        "card:playback_LFE",
    ]);
    let choice = choose_ports(&capture(), &surround, &ports("auto", "auto", "auto"));
    assert_eq!(choice.output_left.as_deref(), Some("card:playback_FL"));
    assert_eq!(choice.output_right.as_deref(), Some("card:playback_FR"));
}

#[test]
fn foreign_ports_drop_own_client_and_mark_physical() {
    let all = [
        "xguitare:out_l",
        "xguitare-123:in",
        "Scarlett:capture_MONO",
        "xguitare-gui:in",
        "Firefox:output_FL",
    ]
    .map(String::from)
    .to_vec();
    let physical = ["Scarlett:capture_MONO".to_string()];
    assert_eq!(
        foreign_ports(&["xguitare", "xguitare-123"], all, &physical),
        [
            AvailablePort {
                name: "Scarlett:capture_MONO".to_string(),
                is_physical: true,
            },
            AvailablePort {
                name: "xguitare-gui:in".to_string(),
                is_physical: false,
            },
            AvailablePort {
                name: "Firefox:output_FL".to_string(),
                is_physical: false,
            },
        ]
    );
}

fn link(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

#[test]
fn connect_plan_replaces_changed_links_only() {
    let own = ports("xguitare:in", "xguitare:out_l", "xguitare:out_r");
    let current = [
        link("old:capture_1", "xguitare:in"),
        link("xguitare:out_l", "card:playback_FL"),
    ];
    let wanted = ports("card:capture_FL", "card:playback_FL", "card:playback_FR");
    assert_eq!(
        connect_plan(&current, &own, &wanted),
        ConnectPlan {
            disconnect: vec![link("old:capture_1", "xguitare:in")],
            connect: vec![
                link("card:capture_FL", "xguitare:in"),
                link("xguitare:out_r", "card:playback_FR"),
            ],
        }
    );
}

#[test]
fn track_inputs_that_are_not_capture_ports_stay_unconnected() {
    let inputs = [
        Some("card:capture_FR".to_string()),
        Some("player:out_left".to_string()),
        None,
    ];
    assert_eq!(
        capture_track_inputs(&inputs, &capture()),
        (
            vec![Some("card:capture_FR".to_string()), None, None],
            vec![
                "track 2 input 'player:out_left' is not a capture port; it stays unconnected"
                    .to_string()
            ],
        )
    );
}
