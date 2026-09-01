use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

pub const SERVICE_NAME: &str = "omarchy-ai-build-orchestrator.service";

pub struct InstallOptions {
    pub engine_binary: PathBuf,
    pub codex_binary: PathBuf,
    pub claude_binary: PathBuf,
    pub gh_binary: PathBuf,
    pub project_roots: Vec<PathBuf>,
}

pub fn install(options: InstallOptions) -> Result<PathBuf> {
    let unit_path = user_unit_path()?;
    let engine = resolve_executable(&options.engine_binary)?;
    let codex = resolve_executable(&options.codex_binary)?;
    let claude = resolve_executable(&options.claude_binary)?;
    let gh = resolve_executable(&options.gh_binary)?;
    let mut arguments = vec![
        "--codex-bin".to_owned(),
        codex.to_string_lossy().into_owned(),
        "--claude-bin".to_owned(),
        claude.to_string_lossy().into_owned(),
        "--gh-bin".to_owned(),
        gh.to_string_lossy().into_owned(),
    ];
    for root in options.project_roots {
        if !root.is_absolute() {
            bail!("projects root must be absolute: {}", root.display());
        }
        arguments.push("--projects-root".to_owned());
        arguments.push(root.to_string_lossy().into_owned());
    }
    let unit = render_unit(&engine, &arguments)?;
    write_unit_atomically(&unit_path, &unit)?;
    run_systemctl(&["daemon-reload"])?;
    run_systemctl(&["enable", SERVICE_NAME])?;
    run_systemctl(&["restart", SERVICE_NAME])?;
    run_systemctl(&["is-active", "--quiet", SERVICE_NAME])?;
    Ok(unit_path)
}

pub fn status() -> Result<()> {
    run_systemctl(&["status", "--no-pager", SERVICE_NAME])
}

pub fn uninstall() -> Result<PathBuf> {
    let unit_path = user_unit_path()?;
    run_systemctl(&["disable", "--now", SERVICE_NAME])?;
    match fs::remove_file(&unit_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("cannot remove {}", unit_path.display()));
        }
    }
    run_systemctl(&["daemon-reload"])?;
    Ok(unit_path)
}

fn user_unit_path() -> Result<PathBuf> {
    let config_home = match env::var_os("XDG_CONFIG_HOME") {
        Some(value) if value.is_empty() => bail!("XDG_CONFIG_HOME is empty"),
        Some(value) => PathBuf::from(value),
        None => {
            let home = env::var_os("HOME").context("HOME is unavailable")?;
            if home.is_empty() {
                bail!("HOME is empty");
            }
            PathBuf::from(home).join(".config")
        }
    };
    if !config_home.is_absolute() {
        bail!("user configuration directory must be absolute");
    }
    Ok(config_home.join("systemd/user").join(SERVICE_NAME))
}

fn resolve_executable(program: &Path) -> Result<PathBuf> {
    let resolved = if program.components().count() > 1 || program.is_absolute() {
        fs::canonicalize(program)
            .with_context(|| format!("cannot resolve executable {}", program.display()))?
    } else {
        let path = env::var_os("PATH").context("PATH is unavailable")?;
        let mut resolved = None;
        for directory in env::split_paths(&path) {
            let candidate = directory.join(program);
            if candidate.is_file() {
                resolved = Some(fs::canonicalize(&candidate).with_context(|| {
                    format!("cannot resolve executable {}", candidate.display())
                })?);
                break;
            }
        }
        resolved.with_context(|| {
            format!("executable is not available on PATH: {}", program.display())
        })?
    };
    let metadata = fs::metadata(&resolved)
        .with_context(|| format!("cannot inspect executable {}", resolved.display()))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        bail!("path is not executable: {}", resolved.display());
    }
    Ok(resolved)
}

fn render_unit(engine: &Path, arguments: &[String]) -> Result<String> {
    let mut command = quote_unit_argument(engine.to_string_lossy().as_ref())?;
    for argument in arguments {
        command.push(' ');
        command.push_str(&quote_unit_argument(argument)?);
    }
    Ok(format!(
        "[Unit]\nDescription=Local software build orchestration engine\n\n\
         [Service]\nType=simple\nExecStart={command}\nRestart=on-failure\nRestartSec=2s\n\n\
         [Install]\nWantedBy=default.target\n"
    ))
}

fn quote_unit_argument(value: &str) -> Result<String> {
    if value.contains(['\n', '\r', '\0']) {
        bail!("service arguments cannot contain control characters");
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "$$")
        .replace('%', "%%");
    Ok(format!("\"{escaped}\""))
}

fn write_unit_atomically(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().context("service path has no parent")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;
    let temporary = parent.join(format!(".{SERVICE_NAME}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("cannot create {}", temporary.display()))?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(error).context("cannot write engine service unit");
    }
    fs::rename(&temporary, path).with_context(|| format!("cannot install {}", path.display()))?;
    Ok(())
}

fn run_systemctl(arguments: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .status()
        .context("cannot start systemctl")?;
    if !status.success() {
        bail!(
            "systemctl --user {} failed with {status}",
            arguments.join(" ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_restartable_user_service_with_absolute_tools() {
        let unit = render_unit(
            Path::new("/home/dev/.local/bin/orchestrator-engine"),
            &[
                "--codex-bin".to_owned(),
                "/home/dev/.local/bin/codex".to_owned(),
                "--projects-root".to_owned(),
                "/home/dev/Projects With Space".to_owned(),
            ],
        )
        .expect("unit should render");

        assert!(
            unit.contains("ExecStart=\"/home/dev/.local/bin/orchestrator-engine\" \"--codex-bin\"")
        );
        assert!(unit.contains("\"/home/dev/Projects With Space\""));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn escapes_systemd_specifiers_and_variables() {
        assert_eq!(
            quote_unit_argument("/tmp/$engine%1").expect("argument should quote"),
            "\"/tmp/$$engine%%1\""
        );
    }

    #[test]
    fn rejects_multiline_service_arguments() {
        assert!(quote_unit_argument("bad\nargument").is_err());
    }
}
