use std::process::Command;
fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tile-ripper"))
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn help_and_actionable_errors_do_not_access_network_or_echo_secrets() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--dry-run"));
    for (args, expected) in [
        (
            vec!["fetch", "--source", "https://secret:password@example.com/"],
            "missing --format",
        ),
        (vec!["plan", "--surprise", "secret"], "unknown option"),
        (vec!["plan", "missing.json"], "cannot read request file"),
        (
            vec!["plan", "examples/weatherbench2.json", "--bbox", "1,2,3,4"],
            "do not mix",
        ),
        (vec!["plan", "--output"], "requires a value"),
    ] {
        let result = run(&args);
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("password"));
    }
}

#[test]
fn existing_output_is_rejected_before_remote_inspection() {
    let result = run(&["fetch", "examples/weatherbench2.json", "--output", "."]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("choose --output NEW_DIRECTORY"));
}
