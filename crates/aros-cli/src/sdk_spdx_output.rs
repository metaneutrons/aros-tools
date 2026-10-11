//! Descriptor-bound, no-clobber output for supplemental SDK metadata.

use rustix::fs::{self as rfs, Mode, OFlags};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

pub(super) struct Destination {
    directory: File,
    output: PathBuf,
    name: String,
}

impl Destination {
    pub(super) fn prepare(parent: &Path, name: &str) -> std::io::Result<Self> {
        aros_common::publication::PortableOutputName::new(name)?;
        let directory = open_directory(parent)?;
        let destination = Self {
            directory,
            output: parent.join(name),
            name: name.to_owned(),
        };
        destination.validate_parent()?;
        Ok(destination)
    }

    pub(super) fn path(&self) -> &Path {
        &self.output
    }

    pub(super) fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.validate_parent()?;
        let fd = rfs::openat(
            &self.directory,
            self.name.as_str(),
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
        )?;
        let mut file = File::from(fd);
        file.write_all(bytes)?;
        file.sync_all()?;
        rfs::fsync(&self.directory)?;
        file.rewind()?;
        let mut measured = Vec::new();
        (&mut file)
            .take(bytes.len() as u64 + 1)
            .read_to_end(&mut measured)?;
        if measured != bytes {
            return Err(std::io::Error::other(
                "SDK SPDX output changed while writing",
            ));
        }
        self.validate_parent()
    }

    fn validate_parent(&self) -> std::io::Result<()> {
        let selected = rfs::fstat(&self.directory)?;
        let current = rfs::fstat(open_directory(
            self.output.parent().expect("prepared output has a parent"),
        )?)?;
        if selected.st_dev != current.st_dev || selected.st_ino != current.st_ino {
            return Err(std::io::Error::other("SDK SPDX output directory changed"));
        }
        Ok(())
    }
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SDK SPDX directory must be absolute",
        ));
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = rfs::open("/", flags, Mode::empty())?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = rfs::openat(&directory, name, flags, Mode::empty())?;
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "SDK SPDX directory must be canonical",
                ));
            }
        }
    }
    Ok(File::from(directory))
}

#[cfg(test)]
mod tests {
    use super::Destination;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn descriptor_output_is_exact_and_never_clobbers() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let destination = Destination::prepare(&root, "sdk.spdx.json").unwrap();
        destination.write(b"exact metadata").unwrap();
        assert_eq!(fs::read(destination.path()).unwrap(), b"exact metadata");
        assert!(destination.write(b"replacement").is_err());
        assert_eq!(fs::read(destination.path()).unwrap(), b"exact metadata");
    }

    #[test]
    fn replaced_parent_never_redirects_output_into_package() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let parent = root.join("stage");
        let package = root.join("package");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&package).unwrap();
        let destination = Destination::prepare(&parent, "sdk.spdx.json").unwrap();
        fs::rename(&parent, root.join("old-stage")).unwrap();
        symlink(&package, &parent).unwrap();
        assert!(destination.write(b"metadata").is_err());
        assert!(!package.join("sdk.spdx.json").exists());
        assert!(!root.join("old-stage/sdk.spdx.json").exists());
        fs::remove_file(&parent).unwrap();
        fs::create_dir(&parent).unwrap();
        assert!(destination.write(b"metadata").is_err());
        assert!(!parent.join("sdk.spdx.json").exists());
    }
}
