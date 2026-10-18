use crate::model::Device;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub version: u32,
    pub devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected_device: Option<Uuid>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            devices: Vec::new(),
            last_connected_device: None,
        }
    }
}

pub struct Store {
    path: PathBuf,
    pub config: Config,
}

impl Store {
    pub fn open() -> Result<Self> {
        let home = std::env::var_os("HOME").context("Could not find your home folder.")?;
        // Keep the original private storage location across the tinyRDP rebrand.
        Self::at(PathBuf::from(home).join("Library/Application Support/MiniRDP/devices.json"))
    }

    pub(crate) fn at(path: PathBuf) -> Result<Self> {
        let config = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Config>(&bytes)
                .with_context(|| format!("Could not read {}. The file has been preserved; repair or move it before restarting tinyRDP.", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(e).with_context(|| format!("Could not read {}", path.display())),
        };
        ensure!(
            config.version == 1,
            "This device file uses an unsupported version. Update tinyRDP to open it."
        );
        let mut ids = HashSet::new();
        for device in &config.devices {
            device
                .validate()
                .context("The saved device file contains an invalid device.")?;
            ensure!(
                ids.insert(device.id),
                "The device file contains duplicate IDs."
            );
        }
        Ok(Self { path, config })
    }

    /// Commit to disk before replacing in-memory state. A failed write leaves both intact.
    pub fn commit(&mut self, devices: Vec<Device>) -> Result<()> {
        let config = Config {
            version: 1,
            last_connected_device: self
                .config
                .last_connected_device
                .filter(|id| devices.iter().any(|device| device.id == *id)),
            devices,
        };
        self.write(config)
    }

    pub fn remember_connection(&mut self, id: Uuid) -> Result<()> {
        ensure!(
            self.config.devices.iter().any(|device| device.id == id),
            "The connected device is no longer saved."
        );
        let mut config = self.config.clone();
        config.last_connected_device = Some(id);
        self.write(config)
    }

    fn write(&mut self, config: Config) -> Result<()> {
        let parent = self.path.parent().context("Invalid settings path.")?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(parent)
            .context("Could not create the tinyRDP settings folder.")?;
        let temporary = parent.join(format!(".devices-{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(&config)?)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.context("Could not save devices.")?;
        self.config = config;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Resolution, SharedFolder};

    #[test]
    fn roundtrip_and_corrupt_file_is_preserved() {
        let dir = std::env::temp_dir().join(format!("minirdp-test-{}", Uuid::new_v4()));
        let path = dir.join("devices.json");
        let mut store = Store::at(path.clone()).unwrap();
        let device = Device {
            name: "Lab".into(),
            host: "192.168.1.2".into(),
            username: "alex".into(),
            invert_scroll: true,
            shared_folders: vec![SharedFolder {
                name: "Projects".into(),
                path: "/Users/test/Offline folder, \"quoted\"".into(),
            }],
            resolution: Some(Resolution {
                width: 2560,
                height: 1440,
            }),
            ..Device::default()
        };
        store.commit(vec![device.clone()]).unwrap();
        assert_eq!(
            Store::at(path.clone()).unwrap().config.devices,
            vec![device]
        );
        // Older device files should still open with automatic resolution and
        // scrolling that follows macOS.
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        legacy["devices"][0]
            .as_object_mut()
            .unwrap()
            .remove("resolution");
        legacy["devices"][0]
            .as_object_mut()
            .unwrap()
            .remove("invert_scroll");
        legacy["devices"][0]
            .as_object_mut()
            .unwrap()
            .remove("shared_folders");
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(
            Store::at(path.clone()).unwrap().config.devices[0].resolution,
            None
        );
        assert!(!Store::at(path.clone()).unwrap().config.devices[0].invert_scroll);
        assert!(
            Store::at(path.clone()).unwrap().config.devices[0]
                .shared_folders
                .is_empty()
        );
        assert_eq!(
            Store::at(path.clone())
                .unwrap()
                .config
                .last_connected_device,
            None
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::write(&path, b"broken json").unwrap();
        assert!(Store::at(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken json");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recent_connection_survives_restart_and_edits_and_clears_on_removal() {
        let dir = std::env::temp_dir().join(format!("minirdp-history-{}", Uuid::new_v4()));
        let path = dir.join("devices.json");
        let mut store = Store::at(path.clone()).unwrap();
        let first = Device {
            name: "Office".into(),
            host: "office.example.com".into(),
            username: "alex".into(),
            ..Device::default()
        };
        let mut second = Device {
            name: "Home".into(),
            host: "home.example.com".into(),
            ..first.clone()
        };
        second.id = Uuid::new_v4();
        store.commit(vec![first.clone(), second.clone()]).unwrap();
        assert!(store.remember_connection(Uuid::new_v4()).is_err());
        assert_eq!(store.config.last_connected_device, None);

        store.remember_connection(first.id).unwrap();
        let mut reopened = Store::at(path.clone()).unwrap();
        assert_eq!(reopened.config.last_connected_device, Some(first.id));
        reopened.remember_connection(second.id).unwrap();
        second.name = "Home renamed".into();
        reopened
            .commit(vec![first.clone(), second.clone()])
            .unwrap();
        let mut reopened = Store::at(path.clone()).unwrap();
        assert_eq!(reopened.config.last_connected_device, Some(second.id));
        assert_eq!(reopened.config.devices[1].name, "Home renamed");

        reopened.commit(vec![first]).unwrap();
        assert_eq!(reopened.config.last_connected_device, None);
        assert_eq!(Store::at(path).unwrap().config.last_connected_device, None);
        fs::remove_dir_all(dir).unwrap();
    }
}
