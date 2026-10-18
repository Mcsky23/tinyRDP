use crate::model::Device;
use anyhow::{Context, Result, bail, ensure};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
};

pub fn backend() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MINIRDP_FREERDP") {
        let path = PathBuf::from(path);
        ensure!(
            is_executable(&path),
            "MINIRDP_FREERDP does not point to an executable file."
        );
        return Ok(path);
    }
    let mut candidates = Vec::new();
    // The bundled SDL frontend preserves aspect ratio and shares its viewport
    // transform with pointer input. Prefer it over Homebrew's stretch renderer.
    if let Ok(executable) = std::env::current_exe() {
        if let Some(contents) = executable.parent().and_then(|p| p.parent()) {
            candidates.push(contents.join("Helpers/sdl-freerdp"));
        }
    }
    #[cfg(debug_assertions)]
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/freerdp-backend/build/bin/sdl-freerdp"),
    );
    candidates.into_iter().find(|p| is_executable(p)).context(
        "The tinyRDP remote desktop helper is missing. Rebuild tinyRDP with:\n\nbash scripts/bundle.sh\n\nFor development, build the helper with python3 scripts/build_backend.py."
    )
}

fn is_executable(path: &std::path::Path) -> bool {
    path.metadata().is_ok_and(|m| {
        #[cfg(unix)]
        {
            m.is_file() && m.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            m.is_file()
        }
    })
}

fn arguments(device: &Device, password: &str, (width, height): (u32, u32)) -> Result<Vec<String>> {
    device.validate()?;
    ensure!(
        width > 0 && height > 0,
        "The display has an invalid resolution."
    );
    ensure!(
        !password.contains(['\n', '\r', '\0']),
        "Passwords cannot contain line breaks or NUL characters."
    );
    ensure!(password.len() <= 4096, "The password is too long.");
    let mut args = vec![
        format!("/v:{}", device.address()),
        format!("/u:{}", device.username),
        format!("/p:{password}"),
        format!("/t:{}", device.name),
        format!("/size:{width}x{height}"),
        // Keep the selected desktop size fixed when resizing or entering full
        // screen, and let SDL scale it locally. Explicit dimensions also
        // preserve it when starting with /f.
        format!("/smart-sizing:{width}x{height}"),
        "-dynamic-resolution".into(),
        "/log-level:ERROR".into(),
        if device.clipboard {
            "+clipboard"
        } else {
            "-clipboard"
        }
        .into(),
    ];
    if !device.domain.is_empty() {
        args.push(format!("/d:{}", device.domain));
    }
    if device.fullscreen {
        args.push("/f".into());
    }
    // Keep FreeRDP's native certificate verification and trust dialog.
    Ok(args)
}

pub fn start(
    executable: PathBuf,
    device: &Device,
    password: &str,
    desktop_size: (u32, u32),
) -> Result<Child> {
    let args = arguments(device, password, desktop_size)?;
    for folder in &device.shared_folders {
        folder.validate_access()?;
    }
    let folders = serde_json::to_string(&device.shared_folders)?;
    let mut command = Command::new(&executable);
    if let Some(contents) = executable.parent().and_then(|parent| parent.parent()) {
        let openssl = contents.join("Resources/OpenSSL");
        if openssl.join("cert.pem").is_file() {
            // Homebrew's OpenSSL has build-machine paths compiled in. Use the
            // app's own roots, config and providers on Macs without Homebrew.
            // Explicit user overrides still take precedence.
            for (name, path) in [
                ("SSL_CERT_FILE", openssl.join("cert.pem")),
                ("OPENSSL_CONF", openssl.join("openssl.cnf")),
                ("OPENSSL_MODULES", contents.join("Frameworks/ossl-modules")),
            ] {
                if std::env::var_os(name).is_none() {
                    command.env(name, path);
                }
            }
        }
    }
    let mut child = command
        // Secrets travel through a private pipe, never argv, a shell or a file.
        .arg("/args-from:stdin")
        // Each connection has its own helper process. Explicitly override any
        // inherited value so one device's scroll preference cannot leak to another.
        .env(
            "MINIRDP_INVERT_SCROLL",
            if device.invert_scroll { "1" } else { "0" },
        )
        // The helper registers these through FreeRDP's drive API. JSON keeps
        // commas, quotes and Unicode in paths intact without command-line CSV.
        .env("MINIRDP_SHARED_FOLDERS", folders)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start FreeRDP.")?;
    let result = (|| -> Result<()> {
        let mut stdin = child
            .stdin
            .take()
            .context("Could not open the connection pipe.")?;
        for arg in &args {
            writeln!(stdin, "{arg}")?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error).context("Could not send the connection details to FreeRDP.");
    }
    Ok(child)
}

/// Runs on a worker thread; caps diagnostics so a long session cannot grow memory.
pub fn wait(mut child: Child, password: &str) -> Result<()> {
    let mut tail = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        let mut chunk = [0u8; 2048];
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    tail.extend_from_slice(&chunk[..n]);
                    if tail.len() > 8192 {
                        tail.drain(..tail.len() - 8192);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    }
    let status = child
        .wait()
        .context("Could not read the connection status.")?;
    if !status.success() {
        let detail = String::from_utf8_lossy(&tail);
        let detail = if password.is_empty() {
            detail.into_owned()
        } else {
            detail.replace(password, "[redacted]")
        };
        let detail: String = detail
            .chars()
            .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
            .collect();
        if detail.trim().is_empty() {
            bail!(
                "The remote session ended ({status}). Check the address, credentials, and whether Remote Desktop is enabled on the device."
            );
        }
        bail!("The remote session ended ({status}).\n\n{}", detail.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device {
            name: "My Mac's test; $(touch nope)".into(),
            host: "::1".into(),
            username: "DOMAIN\\alex".into(),
            ..Device::default()
        }
    }

    #[test]
    fn credentials_and_shell_metacharacters_stay_inside_pipe_arguments() {
        let args = arguments(&device(), "a password: '$(echo nope)'", (1920, 1080)).unwrap();
        assert!(args.contains(&"/v:[::1]:3389".to_string()));
        assert!(args.contains(&"/p:a password: '$(echo nope)'".to_string()));
        assert!(!args.iter().any(|a| a.contains("cert:ignore")));
        assert!(arguments(&device(), "secret\n/cert:ignore", (1920, 1080)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn folder_list_reaches_each_helper_without_path_quoting_or_inheritance() {
        use crate::model::SharedFolder;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("minirdp-shares-{}", uuid::Uuid::new_v4()));
        let folder = dir.join("Project, \"quoted\" 'files' ✓");
        fs::create_dir_all(&folder).unwrap();
        let script = dir.join("fake-freerdp");
        fs::write(
            &script,
            r#"#!/usr/bin/env python3
import json, os, sys
assert sys.argv[1:] == ['/args-from:stdin']
args = sys.stdin.read().splitlines()
expected = int(next(arg[3:] for arg in args if arg.startswith('/p:')))
folders = json.loads(os.environ['MINIRDP_SHARED_FOLDERS'])
assert len(folders) == expected
if folders:
    assert folders[0]['name'] == 'Projects'
    assert 'Project, "quoted" \'files\' ✓' in folders[0]['path']
    assert os.path.isdir(folders[0]['path'])
"#,
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let shared_device = Device {
            shared_folders: vec![SharedFolder {
                name: "Projects".into(),
                path: folder.clone(),
            }],
            ..device()
        };
        for device in [shared_device.clone(), device()] {
            let expected = device.shared_folders.len().to_string();
            let child = start(script.clone(), &device, &expected, (1920, 1080)).unwrap();
            wait(child, &expected).unwrap();
        }
        fs::remove_dir(&folder).unwrap();
        assert!(start(script, &shared_device, "1", (1920, 1080)).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn each_session_receives_its_device_scroll_setting() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("minirdp-scroll-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let script = dir.join("fake-freerdp");
        // The expected session value travels in the existing private pipe. The
        // helper must receive the same value in its own environment each time.
        fs::write(&script, "#!/bin/sh\ncase \"$MINIRDP_INVERT_SCROLL\" in 0|1) ;; *) exit 94 ;; esac\ngrep -q \"^/p:$MINIRDP_INVERT_SCROLL$\" || exit 95\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        for invert_scroll in [false, true, false] {
            let device = Device {
                invert_scroll,
                ..device()
            };
            let expected = if invert_scroll { "1" } else { "0" };
            let child = start(script.clone(), &device, expected, (1920, 1080)).unwrap();
            wait(child, expected).unwrap();
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_receives_secrets_via_stdin_and_reports_errors() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("minirdp-backend-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let script = dir.join("fake-freerdp");
        fs::write(&script, "#!/bin/sh\n[ \"$#\" -eq 1 ] || exit 91\n[ \"$1\" = '/args-from:stdin' ] || exit 92\ngrep -q '^/p:secret$' || exit 93\nprintf 'authentication failed: secret' >&2\nexit 7\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let child = start(script, &device(), "secret", (1920, 1080)).unwrap();
        let error = wait(child, "secret").unwrap_err().to_string();
        assert!(
            error.contains("authentication failed: [redacted]"),
            "{error}"
        );
        assert!(!error.contains("secret"));
        fs::remove_dir_all(dir).unwrap();
    }
}
