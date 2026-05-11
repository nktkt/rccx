//! `rccx` binary entry point.

use std::process::ExitCode;

use rccx_cli::run;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(&args)
}
