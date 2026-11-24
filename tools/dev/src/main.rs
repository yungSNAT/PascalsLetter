//! Developer and CI tools for this repository.
//!
//! `rtk-dev benchmark`: token savings of each rtk filter against the raw command
//! (the CI benchmark job). Run from the repository root; see `benchmark.rs`.

mod benchmark;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("benchmark") if args.len() == 1 => benchmark::run(),
        _ => {
            eprintln!("usage: rtk-dev benchmark");
            ExitCode::from(2)
        }
    }
}
