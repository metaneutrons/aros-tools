//! Fail-closed filesystem reads for verifier coverage inputs.

use std::fs;
use std::path::{Path, PathBuf};

use aros_common::{read_source, DiagnosticStage};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageReadPhase {
    DirectoryRead,
    DirectoryEntryRead,
    EntryTypeRead,
    UnsafeNode,
    DeclarationRead,
    ManualAggregateRead,
    ProvisioningContextRead,
    ReferenceShapeRead,
}

impl CoverageReadPhase {
    const fn label(self) -> &'static str {
        match self {
            Self::DirectoryRead => "read source directory",
            Self::DirectoryEntryRead => "read source directory entry",
            Self::EntryTypeRead => "inspect source entry type without following links",
            Self::UnsafeNode => "validate source entry type without following links",
            Self::DeclarationRead => "read declaration source",
            Self::ManualAggregateRead => "read manual aggregate source",
            Self::ProvisioningContextRead => "read provisioning context source",
            Self::ReferenceShapeRead => "read reference expansion",
        }
    }

    pub const fn diagnostic_stage(self) -> DiagnosticStage {
        match self {
            Self::DirectoryRead
            | Self::DirectoryEntryRead
            | Self::EntryTypeRead
            | Self::UnsafeNode => DiagnosticStage::SourceWalk,
            Self::DeclarationRead
            | Self::ManualAggregateRead
            | Self::ProvisioningContextRead
            | Self::ReferenceShapeRead => DiagnosticStage::Parsing,
        }
    }
}

#[derive(Debug)]
pub struct CoverageReadError {
    pub phase: CoverageReadPhase,
    pub path: PathBuf,
    source: anyhow::Error,
}

impl CoverageReadError {
    pub fn new(
        phase: CoverageReadPhase,
        path: impl Into<PathBuf>,
        source: impl Into<anyhow::Error>,
    ) -> Self {
        Self {
            phase,
            path: path.into(),
            source: source.into(),
        }
    }
}

impl std::fmt::Display for CoverageReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "cannot {} '{}': {:#}",
            self.phase.label(),
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for CoverageReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub type CoverageReadResult<T> = std::result::Result<T, CoverageReadError>;

pub fn read_optional_coverage_source(
    path: &Path,
    phase: CoverageReadPhase,
) -> CoverageReadResult<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(CoverageReadError::new(
                CoverageReadPhase::UnsafeNode,
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "coverage source is not a no-follow regular file",
                ),
            ))
        }
        Ok(_) => read_source(path)
            .map(Some)
            .map_err(|error| CoverageReadError::new(phase, path, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CoverageReadError::new(phase, path, error)),
    }
}

pub fn find_mmakefiles(root: &Path) -> CoverageReadResult<Vec<PathBuf>> {
    find_mmakefiles_with(root, |directory| fs::read_dir(directory))
}

pub fn find_mmakefiles_with<ReadDirectory, Entries>(
    root: &Path,
    mut read_directory: ReadDirectory,
) -> CoverageReadResult<Vec<PathBuf>>
where
    ReadDirectory: FnMut(&Path) -> std::io::Result<Entries>,
    Entries: IntoIterator<Item = std::io::Result<fs::DirEntry>>,
{
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| CoverageReadError::new(CoverageReadPhase::EntryTypeRead, root, error))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(CoverageReadError::new(
            CoverageReadPhase::UnsafeNode,
            root,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "source walk root is not a no-follow directory",
            ),
        ));
    }

    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = read_directory(&dir).map_err(|error| {
            CoverageReadError::new(CoverageReadPhase::DirectoryRead, &dir, error)
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                CoverageReadError::new(CoverageReadPhase::DirectoryEntryRead, &dir, error)
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                CoverageReadError::new(CoverageReadPhase::EntryTypeRead, &path, error)
            })?;
            let name = entry.file_name();
            if file_type.is_dir() {
                if name == "build" || name == ".git" {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                if matches!(name.to_str(), Some("mmakefile" | "mmakefile.src")) {
                    out.push(path);
                }
            } else if matches!(name.to_str(), Some("mmakefile" | "mmakefile.src"))
                || !file_type.is_symlink()
            {
                return Err(CoverageReadError::new(
                    CoverageReadPhase::UnsafeNode,
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "source walk encountered a symlinked mmakefile or special filesystem node",
                    ),
                ));
            }
        }
    }
    out.sort();
    Ok(out)
}
