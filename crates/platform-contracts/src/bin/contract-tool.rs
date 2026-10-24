//! `contract-tool check <old-openapi> <new-openapi>` — exit 1 on breaking changes.
//! `contract-tool digest <dir>` — print the bundle digest for a contract package directory.
//! `contract-tool request <dir>` — print the `POST /v0/contracts` JSON body for a package directory.
//! `contract-tool validate-v02 <dir>` — validate a v0.2 package (`contract.yaml` + referenced documents).
//! `contract-tool request-v02 <dir>` — print the `POST /v0/capability-contracts` JSON body.

use std::{fs, process::ExitCode};

use platform_contracts::{
    ContractBundle, Manifest, compat,
    v02::{BundleV02, ManifestV02},
};

type V02Package = (
    serde_json::Value,
    std::collections::BTreeMap<String, String>,
    BundleV02,
);

/// Load `contract.yaml` and every relative document it references from `dir` (no URL fetching).
fn v02_package(dir: &str) -> Result<V02Package, String> {
    let text = fs::read_to_string(format!("{dir}/contract.yaml"))
        .map_err(|e| format!("{dir}/contract.yaml: {e}"))?;
    let raw: serde_json::Value = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
    let manifest: ManifestV02 = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
    let mut docs = std::collections::BTreeMap::new();
    for i in &manifest.interfaces {
        if let Some(d) = &i.specification.document {
            let key = d.strip_prefix("./").unwrap_or(d).to_string();
            if key.contains("..") || key.starts_with('/') {
                return Err(format!(
                    "document `{d}` must stay inside the package directory"
                ));
            }
            let body = fs::read_to_string(format!("{dir}/{key}"))
                .map_err(|e| format!("{dir}/{key}: {e}"))?;
            docs.insert(key, body);
        }
    }
    let bundle = BundleV02::build(manifest, &docs).map_err(|e| e.to_string())?;
    Ok((raw, docs, bundle))
}

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
        Some(cmd @ ("validate-v02" | "request-v02")) if args.len() == 3 => {
            match v02_package(&args[2]) {
                Ok((manifest, docs, bundle)) => {
                    if cmd == "validate-v02" {
                        println!(
                            "{} {} {}",
                            bundle.manifest.contract_name(),
                            bundle.manifest.version,
                            bundle.digest()
                        );
                    } else {
                        println!(
                            "{}",
                            serde_json::json!({"contract": manifest, "documents": docs, "digest": bundle.digest()})
                        );
                    }
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
                "usage: contract-tool check <old.yaml> <new.yaml> | digest|request <contract-dir> | validate-v02|request-v02 <contract-dir>"
            );
            ExitCode::from(2)
        }
    }
}
