use std::path::{Path, PathBuf};

use xguitare_protocol::default_socket;

#[test]
fn runtime_dir_holds_the_socket_and_the_uid_is_not_asked() {
    let socket = default_socket(Some(Path::new("/run/user/7/xguitare")), || {
        Err::<u32, &str>("uid asked")
    });
    assert_eq!(
        socket,
        Ok(PathBuf::from("/run/user/7/xguitare/xguitare.sock"))
    );
}

#[test]
fn without_a_runtime_dir_the_tmp_fallback_names_the_uid() {
    assert_eq!(
        default_socket(None, || Ok::<u32, ()>(1234)),
        Ok(PathBuf::from("/tmp/xguitare-1234/xguitare.sock"))
    );
}

#[test]
fn a_uid_error_comes_back_unchanged() {
    assert_eq!(
        default_socket(None, || Err::<u32, _>("no uid")),
        Err("no uid")
    );
}
