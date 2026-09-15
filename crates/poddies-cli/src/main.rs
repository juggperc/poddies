//! `poddies-cli` — the plugin authoring tool.
//!
//! ```text
//! poddies-cli new plugin <name> [--lang rust|python] [--dir <path>]
//! poddies-cli dev <plugin-dir>
//! poddies-cli check <plugin-dir>
//! poddies-cli docs
//! ```

mod dev;
mod scaffold;
mod templates;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("new") => scaffold::new_plugin(&args[1..]),
        Some("dev") => dev::run(&args[1..]),
        Some("check") => scaffold::check(&args[1..]),
        Some("docs") => {
            scaffold::print_docs();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("poddies-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") | None => {
            usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command '{other}'\n");
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    println!(
        "poddies-cli — build Poddies plugins

USAGE:
    poddies-cli new plugin <name> [--lang rust|python] [--dir <path>]
    poddies-cli dev <plugin-dir>
    poddies-cli check <plugin-dir>
    poddies-cli docs

EXAMPLES:
    poddies-cli new plugin my-stats --lang rust
    poddies-cli dev ./plugins/my-stats"
    );
}
