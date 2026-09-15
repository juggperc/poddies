//! `poddies-cli new plugin` and `poddies-cli check`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use poddies_plugin_api::manifest::Runtime;

use crate::templates;

/// Absolute path to the SDK crate in this checkout, so a generated Rust plugin
/// builds with no further setup.
pub fn sdk_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|crates| crates.join("poddies-plugin-sdk"))
        .unwrap_or_else(|| PathBuf::from("poddies-plugin-sdk"))
}

/// Absolute path to the bundled Python SDK, if present.
pub fn python_sdk_path() -> Option<PathBuf> {
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join("python");
    candidate.is_dir().then_some(candidate)
}

pub fn new_plugin(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) != Some("plugin") {
        eprintln!("usage: poddies-cli new plugin <name> [--lang rust|python] [--dir <path>]");
        return ExitCode::from(2);
    }

    let mut name: Option<String> = None;
    let mut language = String::from("rust");
    let mut parent: Option<PathBuf> = None;

    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--lang" => {
                index += 1;
                language = args.get(index).cloned().unwrap_or_default();
            }
            "--dir" => {
                index += 1;
                parent = args.get(index).map(PathBuf::from);
            }
            other if other.starts_with("--") => {
                eprintln!("unknown option '{other}'");
                return ExitCode::from(2);
            }
            other => {
                if name.is_some() {
                    eprintln!("unexpected argument '{other}'");
                    return ExitCode::from(2);
                }
                name = Some(other.to_string());
            }
        }
        index += 1;
    }

    let Some(name) = name else {
        eprintln!("a plugin name is required");
        return ExitCode::from(2);
    };

    if !matches!(language.as_str(), "rust" | "python") {
        eprintln!("--lang must be 'rust' or 'python'");
        return ExitCode::from(2);
    }

    let target = parent.unwrap_or_else(|| PathBuf::from("plugins")).join(&name);
    if target.exists() && std::fs::read_dir(&target).map(|mut d| d.next().is_some()).unwrap_or(false)
    {
        eprintln!("{} already exists and is not empty", target.display());
        return ExitCode::from(2);
    }

    let display = title_case(&name);
    let id = format!("dev.example.{}", name.replace('_', "-"));
    let pascal = pascal_case(&name);
    let crate_name = name.clone();

    let tokens: Vec<(&str, String)> = vec![
        ("@@CRATE@@", crate_name.clone()),
        ("@@ID@@", id),
        ("@@DISPLAY@@", display.clone()),
        ("@@STRUCT@@", pascal.clone()),
        ("@@CLASS@@", pascal),
        ("@@SDK@@", sdk_path().to_string_lossy().replace('\\', "/")),
        ("@@LIB@@", format!("{}.dll", name.replace('-', "_"))),
        ("@@SOURCE@@", if language == "rust" { "src/lib.rs" } else { "plugin.py" }.to_string()),
    ];

    let files: Vec<(PathBuf, String)> = if language == "rust" {
        vec![
            (
                target.join("Cargo.toml"),
                render(templates::RUST_CARGO, &tokens),
            ),
            (
                target.join("src").join("lib.rs"),
                render(templates::RUST_LIB, &tokens),
            ),
            (
                target.join("plugin.json"),
                render(templates::RUST_MANIFEST, &tokens),
            ),
            (target.join("README.md"), render(templates::README, &tokens)),
        ]
    } else {
        vec![
            (
                target.join("plugin.py"),
                render(templates::PY_PLUGIN, &tokens),
            ),
            (
                target.join("plugin.json"),
                render(templates::PY_MANIFEST, &tokens),
            ),
            (target.join("README.md"), render(templates::README, &tokens)),
        ]
    };

    for (path, contents) in files {
        if let Some(dir) = path.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            eprintln!("could not create {}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
        if let Err(err) = std::fs::write(&path, contents) {
            eprintln!("could not write {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
        println!("  created {}", path.display());
    }

    println!(
        "\nScaffolded {display} ({language}) in {}\n\nNext:\n  poddies-cli dev {}",
        target.display(),
        target.display()
    );
    ExitCode::SUCCESS
}

pub fn check(args: &[String]) -> ExitCode {
    let Some(dir) = args.first() else {
        eprintln!("usage: poddies-cli check <plugin-dir>");
        return ExitCode::from(2);
    };
    let dir = PathBuf::from(dir);

    let manifest = match poddies_plugin_host::worker::load_manifest(&dir) {
        Ok(manifest) => manifest,
        Err(err) => {
            eprintln!("invalid: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!("id            {}", manifest.id);
    println!("name          {}", manifest.name);
    println!("version       {}", manifest.version);
    println!("protocol      {} -> host {}", manifest.protocol, poddies_plugin_api::PROTOCOL_VERSION);

    let mut ok = true;

    if manifest.is_compatible() {
        println!("compatible    yes");
    } else {
        println!("compatible    NO — major version mismatch");
        ok = false;
    }

    let capabilities = manifest
        .capabilities
        .iter()
        .filter_map(|capability| {
            serde_json::to_value(capability)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
        })
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "capabilities  {}",
        if capabilities.is_empty() {
            "-".to_string()
        } else {
            capabilities
        }
    );

    match &manifest.runtime {
        Runtime::Native { library } => {
            let path = dir.join(library);
            if path.is_file() {
                println!("runtime       native ({library}) found");
            } else {
                println!("runtime       native ({library}) MISSING — build it first");
                ok = false;
            }
        }
        Runtime::Python { entry } => {
            let path = dir.join(entry);
            if path.is_file() {
                println!("runtime       python ({entry}) found");
            } else {
                println!("runtime       python ({entry}) MISSING");
                ok = false;
            }
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

pub fn print_docs() {
    print!("{}", templates::DOCS);
}

fn render(template: &str, tokens: &[(&str, String)]) -> String {
    let mut output = template.to_string();
    for (token, value) in tokens {
        output = output.replace(token, value);
    }
    output
}

fn title_case(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn pascal_case(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_is_sensible() {
        assert_eq!(title_case("my-stats"), "My Stats");
        assert_eq!(pascal_case("my-stats"), "MyStats");
        assert_eq!(pascal_case("discover_openrss"), "DiscoverOpenrss");
    }

    #[test]
    fn tokens_are_fully_substituted() {
        let tokens = vec![
            ("@@CRATE@@", "x".to_string()),
            ("@@DISPLAY@@", "X".to_string()),
        ];
        let rendered = render("@@CRATE@@ @@DISPLAY@@ @@CRATE@@", &tokens);
        assert_eq!(rendered, "x X x");
        assert!(!rendered.contains("@@"));
    }

    #[test]
    fn every_template_token_is_documented() {
        // Guards against a placeholder being added to a template but not to the
        // substitution list in `new_plugin`.
        let known = [
            "@@CRATE@@", "@@ID@@", "@@DISPLAY@@", "@@STRUCT@@", "@@CLASS@@", "@@SDK@@",
            "@@LIB@@", "@@SOURCE@@",
        ];
        let all = format!(
            "{}{}{}{}{}{}",
            templates::RUST_CARGO,
            templates::RUST_LIB,
            templates::PY_PLUGIN,
            templates::RUST_MANIFEST,
            templates::PY_MANIFEST,
            templates::README
        );
        let mut rest = all.as_str();
        while let Some(start) = rest.find("@@") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("@@") else { break };
            let token = &rest[start..start + 2 + end + 2];
            assert!(known.contains(&token), "unknown template token {token}");
            rest = &after[end + 2..];
        }
    }
}
