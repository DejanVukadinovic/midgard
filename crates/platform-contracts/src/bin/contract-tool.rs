//! `contract-tool check <old-openapi> <new-openapi>` — exit 1 on breaking changes.
//! `contract-tool digest <dir>` — print the bundle digest for a contract package directory.
//! `contract-tool request <dir>` — print the `POST /v0/contracts` JSON body for a package directory.

use std::{fs, process::ExitCode};

use platform_contracts::{ContractBundle, Manifest, compat};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("check") if args.len() == 4 => {
            let load = |p: &str| -> Result<serde_json::Value, String> {
                serde_yaml::from_str(&fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?)
                    .map_err(|e| format!("{p}: {e}"))
            };
            let (old, new) = match (load(&args[2]), load(&args[3])) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => {
                    eprintln!("{e}");
                    return ExitCode::from(2);
                }
            };
            let changes = compat::breaking_changes(&old, &new);
            for c in &changes {
                println!("BREAKING: {c}");
            }
            if changes.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some(cmd @ ("digest" | "request")) if args.len() == 3 => {
            let dir = &args[2];
            let run = || -> Result<String, String> {
                let m: Manifest = serde_yaml::from_str(
                    &fs::read_to_string(format!("{dir}/manifest.yaml"))
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                let oas =
                    fs::read_to_string(format!("{dir}/openapi.yaml")).map_err(|e| e.to_string())?;
                let sem =
                    fs::read_to_string(format!("{dir}/semantics.md")).map_err(|e| e.to_string())?;
                let b = ContractBundle::parse(m, oas, sem).map_err(|e| e.to_string())?;
                if cmd == "digest" {
                    Ok(b.digest())
                } else {
                    serde_json::to_string(&serde_json::json!({
                        "manifest": b.manifest, "openapi": b.openapi, "semantics": b.semantics, "digest": b.digest(),
                    }))
                    .map_err(|e| e.to_string())
                }
            };
            match run() {
                Ok(d) => {
                    println!("{d}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(2)
                }
            }
        }
        _ => {
            eprintln!(
                "usage: contract-tool check <old.yaml> <new.yaml> | digest <contract-dir> | request <contract-dir>"
            );
            ExitCode::from(2)
        }
    }
}
