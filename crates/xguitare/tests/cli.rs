use xguitare::run_main;

#[test]
fn run_main_usage_error_exit_1() {
    assert_eq!(run_main(["xguitare", "bogus"]), 1);
}

#[test]
fn run_main_help_exit_0() {
    assert_eq!(run_main(["xguitare", "--help"]), 0);
}
