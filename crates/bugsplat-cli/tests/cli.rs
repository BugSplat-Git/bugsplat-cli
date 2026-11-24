//! Checks on the command tree itself. Each work stream adds its own test file next to this one.

use assert_cmd::Command;
use predicates::prelude::*;

fn bugsplat() -> Command {
    let mut command = Command::cargo_bin("bugsplat").unwrap();
    for name in [
        "BUGSPLAT_ACCESS_TOKEN",
        "BUGSPLAT_API_URL",
        "BUGSPLAT_HOST",
        "BUGSPLAT_DATABASE",
        "BUGSPLAT_CLIENT_ID",
        "BUGSPLAT_CLIENT_SECRET",
        "SYMBOL_UPLOAD_CLIENT_ID",
        "SYMBOL_UPLOAD_CLIENT_SECRET",
    ] {
        command.env_remove(name);
    }
    command
}

#[test]
fn help_lists_the_commands() {
    let assert = bugsplat().arg("--help").assert().success();
    let help = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for command in [
        "login",
        "logout",
        "whoami",
        "databases",
        "reports",
        "groups",
        "versions",
        "defects",
        "symbols",
        "tool",
        "completions",
        "mcp",
    ] {
        assert!(help.contains(command), "--help is missing `{command}`");
    }
}

#[test]
fn symbols_help_lists_its_subcommands() {
    let assert = bugsplat().args(["symbols", "--help"]).assert().success();
    let help = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for command in ["list", "info", "find", "upload", "delete"] {
        assert!(
            help.contains(command),
            "symbols --help is missing `{command}`"
        );
    }
}

#[test]
fn version_prints_the_package_version() {
    bugsplat()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn completions_prints_a_script() {
    bugsplat()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("bugsplat"));
}

#[test]
fn an_unbuilt_command_fails_with_a_link_to_its_issue() {
    bugsplat()
        .arg("mcp")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not implemented yet"))
        .stderr(predicate::str::contains("/issues/9"));
}

#[test]
fn a_bad_api_url_is_a_usage_error() {
    bugsplat()
        .args(["--api-url", "not a url", "mcp"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Invalid API URL"));
}
