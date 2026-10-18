use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::IpAddr, path::PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SharedFolder {
    pub name: String,
    pub path: PathBuf,
}

impl SharedFolder {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.trim().is_empty()
                && self.name == self.name.trim()
                && self.name.chars().count() <= 64
                && !self.name.ends_with('.')
                && !self
                    .name
                    .chars()
                    .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
            "Share names must contain 1–64 characters, without leading/trailing spaces, a trailing dot, or these characters: < > : \" / \\ | ? *"
        );
        ensure!(self.path.is_absolute(), "Choose an absolute folder path.");
        let path = self
            .path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("The folder path must be valid Unicode."))?;
        ensure!(
            !path.contains('\\'),
            "RDP cannot share a folder whose path contains a backslash."
        );
        ensure!(
            path.encode_utf16().count() <= 260,
            "The shared folder path is too long for this RDP backend (260 characters maximum)."
        );
        ensure!(
            !path.chars().any(char::is_control),
            "Folder paths cannot contain control characters."
        );
        Ok(())
    }

    pub fn validate_access(&self) -> Result<()> {
        self.validate()?;
        ensure!(
            self.path.is_dir(),
            "Shared folder '{}' is missing or is not a folder: {}",
            self.name,
            self.path.display()
        );
        std::fs::read_dir(&self.path).map_err(|error| {
            anyhow::anyhow!("Cannot read shared folder '{}': {error}", self.name)
        })?;
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(self.path.as_os_str().as_bytes())?;
            // SAFETY: path is a live NUL-terminated string; access does not modify it.
            let accessible =
                unsafe { libc::access(path.as_ptr(), libc::R_OK | libc::W_OK | libc::X_OK) };
            ensure!(
                accessible == 0,
                "Shared folder '{}' needs read and write access: {}",
                self.name,
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
}

pub fn validate_shared_folders(folders: &[SharedFolder]) -> Result<()> {
    ensure!(folders.len() <= 64, "A device can share up to 64 folders.");
    let mut names = HashSet::new();
    let mut paths = HashSet::new();
    for folder in folders {
        folder.validate()?;
        ensure!(
            names.insert(folder.name.to_lowercase()),
            "Each shared folder needs a unique share name."
        );
        ensure!(
            paths.insert(&folder.path),
            "This folder is already shared with the device."
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub fn validate(self) -> Result<()> {
        ensure!(
            (200..=8192).contains(&self.width) && (200..=8192).contains(&self.height),
            "Resolution width and height must each be between 200 and 8192."
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Device {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default = "enabled")]
    pub remember_password: bool,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default = "enabled")]
    pub clipboard: bool,
    #[serde(default)]
    pub invert_scroll: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_folders: Vec<SharedFolder>,
}

fn enabled() -> bool {
    true
}

impl Default for Device {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            host: String::new(),
            port: 3389,
            username: String::new(),
            domain: String::new(),
            remember_password: true,
            fullscreen: false,
            clipboard: true,
            invert_scroll: false,
            resolution: None,
            shared_folders: Vec::new(),
        }
    }
}

impl Device {
    pub fn validate(&self) -> Result<()> {
        validate_shared_folders(&self.shared_folders)?;
        if let Some(resolution) = self.resolution {
            resolution.validate()?;
        }
        ensure!(!self.id.is_nil(), "Device ID is missing.");
        ensure!(!self.name.trim().is_empty(), "Enter a device name.");
        ensure!(
            self.name.chars().count() <= 100,
            "Device names can contain up to 100 characters."
        );
        ensure!(!self.host.is_empty(), "Enter a hostname or IP address.");
        ensure!(self.host.len() <= 253, "The hostname is too long.");
        ensure!(self.port > 0, "The port must be between 1 and 65535.");
        ensure!(
            !self.username.trim().is_empty(),
            "Enter the remote account username."
        );
        for (label, value) in [
            ("Device name", &self.name),
            ("Hostname", &self.host),
            ("Username", &self.username),
            ("Domain", &self.domain),
        ] {
            ensure!(
                !value.chars().any(char::is_control),
                "{label} cannot contain control characters."
            );
        }
        let host = self
            .host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(&self.host);
        if host != self.host {
            ensure!(
                host.parse::<std::net::Ipv6Addr>().is_ok(),
                "Brackets are only valid around an IPv6 address."
            );
        }
        if host.parse::<IpAddr>().is_err()
            && !host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            })
        {
            bail!(
                "Enter a hostname or IP address without a protocol or port. Use the Port field separately."
            );
        }
        Ok(())
    }

    pub fn address(&self) -> String {
        let host = self.host.trim_start_matches('[').trim_end_matches(']');
        if host.contains(':') {
            format!("[{host}]:{}", self.port)
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_shared_folder_names_and_paths_without_requiring_mounted_folders() {
        let folder = SharedFolder {
            name: "Projects, files ✓".into(),
            path: PathBuf::from("/Users/test/Folder, \"quoted\" 'files' ✓"),
        };
        assert!(folder.validate().is_ok());
        assert!(folder.validate_access().is_err());
        for name in [
            "",
            " ",
            ".",
            "..",
            " trailing ",
            "bad/name",
            "bad\\name",
            "bad:name",
            "bad\nname",
        ] {
            assert!(
                SharedFolder {
                    name: name.into(),
                    ..folder.clone()
                }
                .validate()
                .is_err(),
                "{name:?}"
            );
        }
        for path in ["relative/path", "/tmp/invalid\npath", "/tmp/back\\slash"] {
            assert!(
                SharedFolder {
                    path: path.into(),
                    ..folder.clone()
                }
                .validate()
                .is_err(),
                "{path:?}"
            );
        }
        assert!(
            validate_shared_folders(&[
                folder.clone(),
                SharedFolder {
                    name: "PROJECTS, FILES ✓".into(),
                    path: "/tmp/other".into()
                }
            ])
            .is_err()
        );
        assert!(
            validate_shared_folders(&[
                folder.clone(),
                SharedFolder {
                    name: "Other".into(),
                    ..folder
                }
            ])
            .is_err()
        );
    }

    pub fn device() -> Device {
        Device {
            name: "Work PC".into(),
            host: "office.example.com".into(),
            username: "alex".into(),
            ..Device::default()
        }
    }

    #[test]
    fn accepts_dns_ipv4_and_ipv6() {
        for host in [
            "office.example.com",
            "my-pc",
            "192.168.1.12",
            "::1",
            "[2001:db8::1]",
        ] {
            assert!(
                Device {
                    host: host.into(),
                    ..device()
                }
                .validate()
                .is_ok(),
                "{host}"
            );
        }
        assert_eq!(
            Device {
                host: "::1".into(),
                ..device()
            }
            .address(),
            "[::1]:3389"
        );
    }

    #[test]
    fn rejects_invalid_hosts_and_argument_injection() {
        for host in [
            "",
            "rdp://office",
            "office:3390",
            "a b",
            "host\n/cert:ignore",
            "-bad",
            "[example.com]",
            "bad..host",
        ] {
            assert!(
                Device {
                    host: host.into(),
                    ..device()
                }
                .validate()
                .is_err(),
                "{host}"
            );
        }
        assert!(
            Device {
                username: "user\n/p:injected".into(),
                ..device()
            }
            .validate()
            .is_err()
        );
        assert!(
            Device {
                port: 0,
                ..device()
            }
            .validate()
            .is_err()
        );
    }
}
