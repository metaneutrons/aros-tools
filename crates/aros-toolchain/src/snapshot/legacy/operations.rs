//! Private mutation boundary; production always uses real writes/publication.

use std::{fs::File, io, io::Write as _, path::Path};

use aros_common::publication::{
    publication_failure_class, publish_prepared_source_tree_noclobber, PublicationFailureClass,
};

/// Preserve the publisher's stable outcome without exposing paths or raw errors.
pub(in crate::snapshot) struct PublicationFailure {
    pub class: PublicationFailureClass,
    pub kind: io::ErrorKind,
}

pub(in crate::snapshot) trait Operations {
    fn write_metadata(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()>;
    fn publish(&mut self, staging: &Path, destination: &Path) -> Result<(), PublicationFailure>;
}

pub(in crate::snapshot) struct SystemOperations;

impl Operations for SystemOperations {
    fn write_metadata(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        file.write_all(bytes)
    }

    fn publish(&mut self, staging: &Path, destination: &Path) -> Result<(), PublicationFailure> {
        publish_prepared_source_tree_noclobber(staging, destination)
            .map(|_| ())
            .map_err(|error| PublicationFailure {
                class: publication_failure_class(&error),
                kind: error.kind(),
            })
    }
}
