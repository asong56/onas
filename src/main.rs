//! `onas` CLI binary — a thin wrapper over the `onas` library crate
//! (`src/lib.rs`). Kept thin deliberately: anything another program
//! might want to call directly (rather than spawning `onas` as a
//! subprocess and parsing stdout) belongs in the library, not here.

use clap::Parser;
use onas::cli::{Cli, Command};
use onas::exitcode;

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn")
    ).init();

    let cli = Cli::parse();
    let json = cli.json;

    let result = match cli.command {
        Command::Image(a) => onas::run_image(a),
        Command::Audio(a) => onas::run_audio(a),
        Command::Video(a) => onas::run_video(a),
        Command::Frame(a) => onas::run_frame(a),
        Command::Meta(a)  => onas::run_meta(a),
    };

    match result {
        Ok(report) => {
            if json {
                // The human-readable summary already went to stdout from
                // inside the `run_*` call above; --json prints a second,
                // machine-readable line so scripted callers don't have to
                // choose between readable output and parseable output —
                // pipe stdout through `tail -1 | jq` (or just read the
                // last line) to get the structured version.
                match serde_json::to_string(&report) {
                    Ok(line) => println!("{line}"),
                    Err(e)   => log::warn!("--json: failed to serialize report: {e}"),
                }
            }
            std::process::exit(exitcode::OK);
        }
        Err(err) => {
            let code = exitcode::classify(&err);
            if json {
                let obj = serde_json::json!({
                    "error": format!("{:#}", err),
                    "exit_code": code,
                });
                eprintln!("{obj}");
            } else {
                eprintln!("error: {err:#}");
            }
            std::process::exit(code);
        }
    }
}
