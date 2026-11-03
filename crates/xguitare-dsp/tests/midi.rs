use xguitare_dsp::midi::program_change;

#[test]
fn program_change_parsed() {
    assert_eq!(program_change(&[0xC0, 5]), Some(5));
}

#[test]
fn channel_ignored() {
    assert_eq!(program_change(&[0xC3, 7]), Some(7));
}

#[test]
fn note_on_is_none() {
    assert_eq!(program_change(&[0x90, 60, 100]), None);
}

#[test]
fn short_message_is_none() {
    assert_eq!(program_change(&[0xC0]), None);
}
