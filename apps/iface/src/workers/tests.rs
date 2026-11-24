use super::*;
#[test]
fn worker_options_do_not_admit_grants_from_mcp_clients() {
    let options = Options::parse(
        &[
            "mcp",
            "--root",
            "/tmp/worker-test",
            "--allow-contributor-data",
        ]
        .map(String::from),
    );
    assert!(options.is_err());
    assert!(Options::parse(&["serve", "--root", "relative"].map(String::from)).is_err());
}
