use xguitare::run_main;

#[test]
fn run_main_usage_error_exit_1() {
    assert_eq!(run_main(["xguitare", "bogus"]), 1);
}

#[test]
fn run_main_help_exit_0() {
    assert_eq!(run_main(["xguitare", "--help"]), 0);
}

#[test]
fn run_main_no_args_is_usage_error() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_xguitare"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr
            .lines()
            .next()
            .unwrap_or_default()
            .starts_with("error:"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
}
