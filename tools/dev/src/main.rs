//! Developer and CI tools for this repository.
//!
//! `rtk-dev benchmark`: token savings of each rtk filter against the raw command
//! (the CI benchmark job). Run from the repository root; see `benchmark.rs`.
//! `rtk-dev check-test-presence [BASE | --self-test]`: changed `*_cmd.rs` files
//! must carry tests (the CI test presence job); see `test_presence.rs`.
//! `rtk-dev smoke`: runs every rtk command once against this repository with
//! the installed `rtk`; see `smoke.rs`.

mod benchmark;
mod smoke;
mod test_presence;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("benchmark") if args.len() == 1 => benchmark::run(),
        Some("check-test-presence") => test_presence::run(&args[1..]),
        Some("smoke") if args.len() == 1 => smoke::run(),
        _ => {
            eprintln!(
                "usage: rtk-dev benchmark | rtk-dev check-test-presence [BASE | --self-test] | rtk-dev smoke"
            );
            ExitCode::from(2)
        }
    }
}
