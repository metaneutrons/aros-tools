//! Exact raw inputs for source-owned host-C generators.

use super::native_contract::NativeContractSelection;
use aros_common::native_host_generator::NativeHostFileInput;
use miette::{IntoDiagnostic, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(super) async fn prepare(
    contract: Option<&NativeContractSelection>,
    offline: bool,
) -> Result<Option<PathBuf>> {
    let Some(contract) = contract else {
        return Ok(None);
    };
    let inputs: BTreeMap<_, _> = contract
        .host_file_generators()
        .iter()
        .flat_map(|generator| &generator.inputs)
        .map(|input| (input.filename.clone(), input.clone()))
        .collect();
    if inputs.is_empty() {
        return Ok(None);
    }
    let identity = aros_common::sha256_bytes(&serde_json::to_vec(&inputs).into_diagnostic()?);
    let root = aros_cache::archive_cache_root()
        .map_err(|error| miette::miette!(error))?
        .join("native-host-files-v1")
        .join(identity.to_string());
    aros_common::ensure_directory_nofollow(&root).into_diagnostic()?;
    for input in inputs.values() {
        verify_or_acquire(&root, input, offline).await?;
    }
    Ok(Some(root.canonicalize().into_diagnostic()?))
}

async fn verify_or_acquire(
    root: &std::path::Path,
    input: &NativeHostFileInput,
    offline: bool,
) -> Result<()> {
    let snapshot = aros_fetch::engine::cache::acquire_https_cache_payload(
        root,
        &input.filename,
        &input.url,
        input.size,
        &input.sha256,
        offline,
    )
    .await
    .map_err(|error| miette::miette!("sealed host input {}: {error}", input.filename))?;
    snapshot.revalidate().map_err(|error| {
        miette::miette!("sealed host input {} changed: {error}", input.filename)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn offline_raw_inputs_reject_absence_corruption_and_symlinks() {
        let cache = tempfile::tempdir().unwrap();
        let bytes = b"exact host input\n";
        let input = NativeHostFileInput {
            filename: "data.txt".into(),
            url: "https://example.invalid/1/data.txt".into(),
            size: bytes.len() as u64,
            sha256: aros_common::sha256_bytes(bytes),
        };
        assert!(verify_or_acquire(cache.path(), &input, true).await.is_err());
        std::fs::write(cache.path().join("data.txt"), bytes).unwrap();
        verify_or_acquire(cache.path(), &input, true).await.unwrap();
        std::fs::write(cache.path().join("data.txt"), b"changed").unwrap();
        assert!(verify_or_acquire(cache.path(), &input, true).await.is_err());
        std::fs::remove_file(cache.path().join("data.txt")).unwrap();
        std::fs::write(cache.path().join("other.txt"), bytes).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("other.txt", cache.path().join("data.txt")).unwrap();
            assert!(verify_or_acquire(cache.path(), &input, true).await.is_err());
        }
    }
}
