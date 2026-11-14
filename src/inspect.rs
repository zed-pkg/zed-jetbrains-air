use crate::model::{
    CliState, PackageState, ProjectReport, Severity, actions_for, diagnostic, summarize,
};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

pub fn project(start: &Path) -> Result<ProjectReport> {
    let start = fs::canonicalize(start)
        .with_context(|| format!("project path does not exist: {}", start.display()))?;
    let root = discover_root(&start);
    let manifest_path = root.join(".zpkg.toml");
    let lock_path = root.join(".zpkg.lock");
    let modules_path = root.join("zed_modules");
    let manifest_regular = regular_marker(&manifest_path);
    let lock_regular = regular_marker(&lock_path);
    let cli = inspect_cli();
    let mut package = PackageState::default();
    let mut diagnostics = Vec::new();

    if !cli.available {
        diagnostics.push(diagnostic(
            "ZED001",
            Severity::Error,
            "The `zed` CLI is not available on PATH.",
            "Install zed-cli, restart Air so its environment sees the updated PATH, and inspect again.",
        ));
    }

    let manifest_value = if manifest_regular {
        package.manifest_path = Some(path_text(&manifest_path));
        match parse_toml(&manifest_path) {
            Ok(value) => {
                package.manifest_valid = true;
                package.org = nested_string(&value, &["package", "org"]);
                package.name = nested_string(&value, &["package", "name"]);
                package.version = nested_string(&value, &["package", "version"]);
                package.direct_dependency_count = value
                    .get("dependencies")
                    .and_then(toml::Value::as_table)
                    .map(|table| table.len())
                    .unwrap_or(0);
                Some(value)
            }
            Err(error) => {
                diagnostics.push(diagnostic(
                    "ZED005",
                    Severity::Error,
                    format!("The Zed manifest is invalid TOML: {error}"),
                    "Fix `.zpkg.toml`; do not regenerate the lockfile until the manifest parses cleanly.",
                ));
                None
            }
        }
    } else if marker_present(&manifest_path) {
        diagnostics.push(diagnostic(
            "ZED005",
            Severity::Error,
            "The Zed manifest marker is not a local regular file.",
            "Replace the symlink or directory with a reviewed local .zpkg.toml file.",
        ));
        None
    } else {
        diagnostics.push(diagnostic(
            "ZED002",
            Severity::Warning,
            "No `.zpkg.toml` manifest was found for this workspace.",
            "Run `zed init` for an authored package, or `zed install <org>/<name>@<requirement>` to adopt Zed while adding a dependency.",
        ));
        None
    };

    let lock_value = if lock_regular {
        package.lock_path = Some(path_text(&lock_path));
        match parse_toml(&lock_path) {
            Ok(value) => {
                package.lock_valid = true;
                Some(value)
            }
            Err(error) => {
                diagnostics.push(diagnostic(
                    "ZED006",
                    Severity::Error,
                    format!("The Zed lockfile is invalid TOML: {error}"),
                    "Restore the lockfile from version control or resolve from a valid manifest with `zed install`.",
                ));
                None
            }
        }
    } else {
        if marker_present(&lock_path) {
            diagnostics.push(diagnostic(
                "ZED006",
                Severity::Error,
                "The Zed lock marker is not a local regular file.",
                "Replace the symlink or directory with a reviewed local .zpkg.lock file.",
            ));
        } else if manifest_regular {
            diagnostics.push(diagnostic(
                "ZED003",
                Severity::Warning,
                "The project has a manifest but no `.zpkg.lock`.",
                "Run `zed install` to resolve dependencies and create the lockfile.",
            ));
        }
        None
    };

    if lock_regular && !marker_present(&manifest_path) {
        diagnostics.push(diagnostic(
            "ZED004",
            Severity::Warning,
            "A lockfile exists without a manifest, so direct dependency intent cannot be reconstructed.",
            "Restore `.zpkg.toml` when possible. For an intentional lock-only restore use `zed install --frozen --do-not-write-new-manifest`.",
        ));
    }

    if fs::symlink_metadata(&modules_path).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        package.modules_path = Some(path_text(&modules_path));
    } else if manifest_value.is_some() && lock_value.is_some() {
        diagnostics.push(diagnostic(
            "ZED007",
            Severity::Warning,
            "The manifest and lockfile exist, but `zed_modules/` is not materialized.",
            "Run `zed install --frozen` to restore the exact locked packages.",
        ));
    }

    if manifest_value.is_some()
        && lock_value.is_some()
        && is_newer(&manifest_path, &lock_path).unwrap_or(false)
    {
        diagnostics.push(diagnostic(
            "ZED008",
            Severity::Info,
            "The manifest is newer than the lockfile; dependency intent may have changed since the last resolution.",
            "Run `zed install` and review the lockfile diff.",
        ));
    }

    if package.manifest_valid
        && (package.org.is_none() || package.name.is_none() || package.version.is_none())
    {
        diagnostics.push(diagnostic(
            "ZED009",
            Severity::Warning,
            "The manifest parses, but one or more `[package]` identity fields are missing.",
            "Set `package.org`, `package.name`, and `package.version` before publishing.",
        ));
    }

    if diagnostics.is_empty() {
        diagnostics.push(diagnostic(
            "ZED000",
            Severity::Info,
            "No obvious Zed package-state problems were detected.",
            "Use `zed install --frozen` in CI and `zed store status` when investigating local cache behavior.",
        ));
    }

    let recommended_actions = actions_for(&diagnostics);
    let summary = summarize(&diagnostics);

    Ok(ProjectReport {
        root: path_text(&root),
        cli,
        package,
        summary,
        diagnostics,
        recommended_actions,
    })
}

fn discover_root(start: &Path) -> PathBuf {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start).to_path_buf()
    } else {
        start.to_path_buf()
    };
    let fallback = current.clone();

    loop {
        if marker_present(&current.join(".zpkg.toml"))
            || marker_present(&current.join(".zpkg.lock"))
        {
            return current;
        }
        if !current.pop() {
            return fallback;
        }
    }
}

// Never follow symlinks while deciding which local package files are trusted.
// Include malformed markers during discovery so diagnostics stay at their root.
fn marker_present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn regular_marker(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn inspect_cli() -> CliState {
    match Command::new("zed").arg("--version").output() {
        Ok(output) if output.status.success() => CliState {
            available: true,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
            error: None,
        },
        Ok(output) => CliState {
            available: false,
            version: None,
            error: Some(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
        },
        Err(error) => CliState {
            available: false,
            version: None,
            error: Some(error.to_string()),
        },
    }
}

fn parse_toml(path: &Path) -> std::result::Result<toml::Value, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    toml::from_str(&text).map_err(|error| error.to_string())
}

fn nested_string(value: &toml::Value, keys: &[&str]) -> Option<String> {
    let mut current = value;
    for key in keys {
        current = current.get(*key)?;
    }
    current.as_str().map(ToOwned::to_owned)
}

fn is_newer(left: &Path, right: &Path) -> Result<bool> {
    let left_modified = fs::metadata(left)?
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let right_modified = fs::metadata(right)?
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(left_modified > right_modified)
}

pub fn render(report: &ProjectReport) -> String {
    let mut output = format!(
        "Zed package status for {}\nerrors: {}, warnings: {}, info: {}\n",
        report.root, report.summary.errors, report.summary.warnings, report.summary.info
    );

    for diagnostic in &report.diagnostics {
        output.push_str(&format!(
            "\n[{:?}] {}: {}\n  Resolution: {}\n",
            diagnostic.severity, diagnostic.code, diagnostic.message, diagnostic.resolution
        ));
    }

    output.push_str("\nRecommended actions:\n");
    for action in &report.recommended_actions {
        output.push_str(&format!("- {}", action.title));
        if let Some(command) = &action.command {
            output.push_str(&format!(": `{command}`"));
        }
        output.push('\n');
    }
    output
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write as _;

    #[test]
    fn reports_missing_lock() {
        let directory = tempfile::tempdir().unwrap();
        let mut manifest = File::create(directory.path().join(".zpkg.toml")).unwrap();
        writeln!(
            manifest,
            "[package]\norg = \"acme\"\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nfoo = \"^1\""
        )
        .unwrap();

        let report = project(directory.path()).unwrap();
        assert_eq!(report.package.direct_dependency_count, 1);
        assert!(report.diagnostics.iter().any(|item| item.code == "ZED003"));
    }

    #[test]
    fn forged_manifest_directory_is_not_parsed_or_admitted() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join(".zpkg.toml")).unwrap();
        let report = project(directory.path()).unwrap();
        assert_eq!(Path::new(&report.root), fs::canonicalize(directory.path()).unwrap());
        assert!(!report.package.manifest_valid);
        assert!(report.diagnostics.iter().any(|item| item.code == "ZED005"));
    }

    #[cfg(unix)]
    #[test]
    fn package_markers_and_materialization_do_not_follow_external_symlinks() {
        use std::os::unix::fs::symlink;
        let project_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(
            outside.path().join("manifest.toml"),
            r#"[package]
org = "outside"
name = "leak"
version = "1.0.0"
"#,
        )
        .unwrap();
        fs::write(outside.path().join("lock.toml"), "version = 1\n").unwrap();
        fs::create_dir(outside.path().join("modules")).unwrap();
        symlink(
            outside.path().join("manifest.toml"),
            project_dir.path().join(".zpkg.toml"),
        )
        .unwrap();
        symlink(
            outside.path().join("lock.toml"),
            project_dir.path().join(".zpkg.lock"),
        )
        .unwrap();
        symlink(
            outside.path().join("modules"),
            project_dir.path().join("zed_modules"),
        )
        .unwrap();
        let report = project(project_dir.path()).unwrap();
        assert!(!report.package.manifest_valid);
        assert!(!report.package.lock_valid);
        assert!(report.package.modules_path.is_none());
        assert!(report.diagnostics.iter().any(|item| item.code == "ZED005"));
        assert!(report.diagnostics.iter().any(|item| item.code == "ZED006"));
        // The outside content must never acquire the local package's identity.
        assert_ne!(report.package.org.as_deref(), Some("outside"));
    }

    #[test]
    fn reports_any_missing_identity_field() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".zpkg.toml"),
            "[package]\norg = \"acme\"\nname = \"demo\"\n",
        )
        .unwrap();

        let report = project(directory.path()).unwrap();
        assert!(report.diagnostics.iter().any(|item| item.code == "ZED009"));
    }
}
