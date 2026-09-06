//! Shared no-follow descriptor traversal for inspection and work ownership.

use std::fs::File;
use std::io;
use std::path::{Component, Path};

use rustix::fs::{self as fs, Mode, OFlags};

pub const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

pub fn open_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory must be absolute",
        ));
    }
    let mut file = File::from(fs::open("/", DIRECTORY, Mode::empty())?);
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                file = File::from(fs::openat(&file, name, DIRECTORY, Mode::empty())?);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "directory contains noncanonical components",
                ))
            }
        }
    }
    Ok(file)
}
