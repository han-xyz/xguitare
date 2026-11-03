mod common;

use common::{TempDir, serve};
use xguitare_client::{Transport, UnixSocketTransport};
use xguitare_core::control::{Request, Response};

#[test]
fn one_shot_round_trip() {
    let dir = TempDir::new("one-shot");
    let server = serve(&dir.socket(), &[r#"{"ok":true,"path":"/bank/clean.toml"}"#]);
    let mut transport = UnixSocketTransport::from_config(&dir.config()).unwrap();
    let response = transport.request(&Request::Save).unwrap();
    assert_eq!(
        response,
        Response::from_line(r#"{"ok":true,"path":"/bank/clean.toml"}"#).unwrap()
    );
    server.join().unwrap();
}
