use xguitare_core::Config;

#[test]
fn defaults_when_missing() {
    let home = std::env::temp_dir().join(format!("xguitare-config-{}-home", std::process::id()));
    // SAFETY: this is the only test in this binary, so no other thread reads the environment.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &home) };
    let config = Config::load(None).unwrap();
    assert_eq!(config.buffer_size, 128);
    assert_eq!(config.crossfade_ms, 20.0);
    assert_eq!(config.ir_max_ms, 100.0);
    assert_eq!(config.ports.input, "auto");
    assert_eq!(config.ports.output_left, "auto");
    assert_eq!(config.ports.output_right, "auto");
    assert_eq!(config.bank_dir, home.join("xguitare/bank"));
    assert!(config.socket.ends_with("xguitare.sock"));
}
