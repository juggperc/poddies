//! Standalone worker entry point.
//!
//! In a release build the app runs this mode from its own binary (`poddies.exe
//! --plugin-worker <dir>`), so the shipped product stays a single executable.
//! This separate binary exists for plugin development and for `poddies dev`.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Accept both `--plugin-worker <dir>` and a bare `<dir>`.
    let dir = match args.as_slice() {
        [flag, dir] if flag == "--plugin-worker" => PathBuf::from(dir),
        [dir] => PathBuf::from(dir),
        _ => {
            eprintln!("usage: poddies-plugin-worker <plugin-dir>");
            return ExitCode::from(2);
        }
    };

    match poddies_plugin_host::run_worker(&dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("[poddies] worker failed: {err}");
            ExitCode::FAILURE
        }
    }
}
