use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;
use iherb_cli::app;
use iherb_cli::cli::Cli;

/// Parse the command line, run, and report the outcome as an exit code.
///
/// `argv` is read twice on purpose (#9). Clap fails before the parsed struct
/// exists, so a command line it rejects can never be asked whether it wanted
/// JSON; the only way for a parse error to honour `--json` is to look for the
/// flag before parsing.
#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let json = app::wants_json(&args);

    match Cli::try_parse_from(&args) {
        Ok(cli) => app::run(cli).await,
        Err(e) => app::report_clap_error(e, json),
    }
}
