//! Bounded raw TAR/PAX scanning shared by payload readback and extraction.

use super::*;

#[derive(Debug, Default)]
struct PaxOverrides {
    path: Option<Vec<u8>>,
    linkpath: Option<Vec<u8>>,
}

pub(super) fn scan_archive<C: ArchiveConsumer>(
    input: File,
    contract: PayloadArchiveContract<'_>,
    forbidden_prefixes: &[PathBuf],
    consumer: &mut C,
) -> Result<ArchiveTree, ContractError> {
    validate_payload_archive_contract(contract)?;
    let metadata = input
        .metadata()
        .map_err(|_| ContractError::verification("cannot inspect payload archive input"))?;
    if !metadata.is_file() || metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(ContractError::verification(
            "payload archive is not a regular file within the configured resource limit",
        ));
    }
    let archive_size = metadata.len();
    let stream = xz2::stream::Stream::new_stream_decoder(MAX_XZ_DECODER_MEMORY, 0)
        .map_err(|_| ContractError::verification("cannot initialize bounded package XZ decoder"))?;
    let decoder = XzDecoder::new_stream(input, stream);
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    let mut directories = BTreeSet::from([PathBuf::new()]);
    let mut previous = None;
    let mut embedded_manifest = None;
    let mut toolchain_tools = None;
    let mut budget = ArchiveBudget::default();
    let mut pending_pax = None;
    let mut total_pax_bytes = 0_u64;
    let mut logical_index = 0_u64;
    for entry in archive
        .entries()
        .map_err(|_| ContractError::verification("cannot read package tar stream"))?
        .raw(true)
    {
        let mut entry = entry.map_err(|_| {
            ContractError::verification("package tar stream contains an invalid entry")
        })?;
        let header = entry.header().clone();
        let entry_type = header.entry_type();

        validate_tar_header_string_fields(&header)?;
        if entry_type.is_pax_global_extensions()
            || entry_type.is_gnu_longname()
            || entry_type.is_gnu_longlink()
        {
            return Err(ContractError::verification(
                "package tar contains an unsupported global or GNU extension",
            ));
        }
        if entry_type.is_pax_local_extensions() {
            if logical_index == 0 || pending_pax.is_some() {
                return Err(ContractError::verification(
                    "package tar contains a misplaced or duplicate PAX extension",
                ));
            }
            let size = header.size().map_err(|_| {
                ContractError::verification("package PAX header has an invalid size")
            })?;
            if entry.size() != size {
                return Err(ContractError::verification(
                    "package PAX size differs from its raw header",
                ));
            }
            verify_pax_header(&header, size)?;
            account_pax_extension(&mut total_pax_bytes, size)?;
            let bytes = read_exact_entry(&mut entry, size)?;
            pending_pax = Some(parse_pax_overrides(&bytes)?);
            continue;
        }

        let size = header
            .size()
            .map_err(|_| ContractError::verification("package tar entry has an invalid size"))?;
        if entry.size() != size {
            return Err(ContractError::verification(
                "package tar entry size differs from its raw header",
            ));
        }
        budget.account(
            entry_type.as_byte() == b'0',
            size,
            contract.max_entries,
            contract.max_regular_file_bytes,
        )?;
        let pax = pending_pax.take();
        if pax
            .as_ref()
            .is_some_and(|pax| pax.linkpath.is_some() && entry_type.as_byte() != b'2')
        {
            return Err(ContractError::verification(
                "package tar PAX linkpath does not describe a symbolic link",
            ));
        }
        let raw_path = header.path_bytes();
        let path_bytes = pax
            .as_ref()
            .and_then(|pax| pax.path.as_deref())
            .unwrap_or_else(|| raw_path.as_ref());
        validate_canonical_archive_path(path_bytes, entry_type.as_byte(), contract.archive_root)?;
        let path_text = std::str::from_utf8(path_bytes)
            .map_err(|_| ContractError::verification("package tar entry path is not UTF-8"))?;
        let (relative, is_root) =
            package_relative_path(Path::new(path_text), contract.archive_root)?;
        if logical_index == 0 {
            if !is_root || entry_type.as_byte() != b'5' {
                return Err(ContractError::verification(
                    "package tar must begin with the toolchain root directory",
                ));
            }
            verify_header(&header, contract.source_date_epoch, Some(0o755), 0)?;
            logical_index += 1;
            continue;
        }
        logical_index = logical_index
            .checked_add(1)
            .ok_or_else(|| ContractError::verification("package tar entry count overflowed"))?;
        if is_root {
            return Err(ContractError::verification(
                "package tar contains a duplicate toolchain root directory",
            ));
        }
        let relative = relative.ok_or_else(|| {
            ContractError::verification("package tar entry has no relative payload path")
        })?;
        let relative_text = relative
            .to_str()
            .ok_or_else(|| ContractError::verification("package tar entry path is not UTF-8"))?
            .replace('\\', "/");
        if previous
            .as_ref()
            .is_some_and(|previous: &String| previous >= &relative_text)
        {
            return Err(ContractError::verification(
                "package tar members are not in strict lexical order",
            ));
        }
        previous = Some(relative_text.clone());
        let collision = payload_casefold_path_key(&relative)
            .map_err(|_| ContractError::verification("package tar entry path is not portable"))?;
        if !seen.insert(collision) {
            return Err(ContractError::verification(
                "package tar contains a duplicate or case-folding path collision",
            ));
        }
        let parent = relative.parent().ok_or_else(|| {
            ContractError::verification("package tar entry has no relative parent")
        })?;
        if !directories.contains(parent) {
            return Err(ContractError::verification(
                "package tar entry has no previously declared real parent directory",
            ));
        }
        match entry_type.as_byte() {
            b'5' => {
                verify_header(&header, contract.source_date_epoch, Some(0o755), 0)?;
                consumer.directory(&relative)?;
                directories.insert(relative.clone());
                entries.push(directory_entry(relative_text));
            }
            b'2' => {
                verify_header(&header, contract.source_date_epoch, Some(0o777), 0)?;
                let raw_target = header.link_name_bytes();
                let target_bytes = pax
                    .as_ref()
                    .and_then(|pax| pax.linkpath.as_deref())
                    .or(raw_target.as_deref())
                    .ok_or_else(|| {
                        ContractError::verification("package tar symlink has no target")
                    })?;
                if target_bytes.is_empty()
                    || target_bytes.contains(&0)
                    || target_bytes.contains(&b'\\')
                {
                    return Err(ContractError::verification(
                        "package tar symlink target is not canonical",
                    ));
                }
                let target_text = std::str::from_utf8(target_bytes).map_err(|_| {
                    ContractError::verification("package tar symlink target is not UTF-8")
                })?;
                let target = Path::new(target_text);
                validate_link_target(&relative, target)?;
                consumer.symlink(&relative, target)?;
                entries.push(ArosToolchainManifestEntry {
                    path: relative_text,
                    mode: "0777".into(),
                    kind: "symlink".into(),
                    sha256: None,
                    size: None,
                    target: Some(target_text.into()),
                });
            }
            b'0' => {
                verify_header(&header, contract.source_date_epoch, None, size)?;
                if relative == Path::new(contract.manifest_file) {
                    if embedded_manifest.is_some() || size > MAX_METADATA_BYTES {
                        return Err(ContractError::verification(
                            "package tar embedded manifest is missing, duplicated, or too large",
                        ));
                    }
                    let bytes = consumer.embedded_manifest(&relative, &mut entry, size)?;
                    embedded_manifest = Some(bytes);
                } else {
                    let mode = header.mode().map_err(|_| {
                        ContractError::verification("package tar entry has an invalid mode")
                    })?;
                    let mode = if mode & 0o111 == 0 { "0644" } else { "0755" };
                    let sha256 = if contract.capture_toolchain_tools
                        && relative == Path::new(TOOLCHAIN_TOOLS_FILE)
                    {
                        if size > MAX_TOOLCHAIN_TOOLS_BYTES || toolchain_tools.is_some() {
                            return Err(ContractError::verification(
                                "package tar toolchain tools contract is duplicated or too large",
                            ));
                        }
                        let bytes = read_exact_entry(&mut entry, size)?;
                        let mut cursor = io::Cursor::new(bytes.as_slice());
                        let sha256 = consumer.regular(
                            &relative,
                            mode,
                            &mut cursor,
                            size,
                            forbidden_prefixes,
                        )?;
                        toolchain_tools = Some(bytes);
                        sha256
                    } else {
                        consumer.regular(&relative, mode, &mut entry, size, forbidden_prefixes)?
                    };
                    entries.push(ArosToolchainManifestEntry {
                        path: relative_text,
                        mode: mode.into(),
                        kind: "file".into(),
                        sha256: Some(sha256.to_string()),
                        size: Some(size),
                        target: None,
                    });
                }
            }
            _ => {
                return Err(ContractError::verification(
                    "package tar contains an unsupported entry type",
                ));
            }
        }
    }
    if pending_pax.is_some() {
        return Err(ContractError::verification(
            "package tar ends with an orphaned PAX extension",
        ));
    }
    let mut decoder = archive.into_inner();
    verify_archive_terminal(&mut decoder)?;
    if decoder.total_in() != archive_size {
        return Err(ContractError::verification(
            "package XZ stream has trailing bytes or concatenated streams",
        ));
    }
    let embedded_manifest = embedded_manifest.ok_or_else(|| {
        ContractError::verification("payload tar does not contain its embedded manifest")
    })?;
    let tree_sha256 = toolchain_inventory_sha256(&entries)
        .map_err(|_| ContractError::verification("cannot serialize package tar inventory"))?;
    Ok(ArchiveTree {
        embedded_manifest,
        entries,
        tree_sha256,
        toolchain_tools,
    })
}

fn validate_tar_header_string_fields(header: &tar::Header) -> Result<(), ContractError> {
    let bytes = header.as_bytes();
    let name_is_canonical = has_only_trailing_nuls(&bytes[..100]);
    let link_is_canonical = has_only_trailing_nuls(&bytes[157..257]);
    let prefix_is_canonical = header
        .as_ustar()
        .is_none_or(|_| has_only_trailing_nuls(&bytes[345..500]));
    let path = header.path_bytes();
    let link = header.link_name_bytes();
    if !name_is_canonical
        || !link_is_canonical
        || !prefix_is_canonical
        || path.contains(&0)
        || path.contains(&b'\\')
        || link
            .as_ref()
            .is_some_and(|value| value.contains(&0) || value.contains(&b'\\'))
    {
        return Err(ContractError::verification(
            "package tar header contains a noncanonical path or link field",
        ));
    }
    Ok(())
}

fn has_only_trailing_nuls(field: &[u8]) -> bool {
    field
        .iter()
        .position(|byte| *byte == 0)
        .is_none_or(|first_nul| field[first_nul..].iter().all(|byte| *byte == 0))
}

fn verify_pax_header(header: &tar::Header, expected_size: u64) -> Result<(), ContractError> {
    let path = header.path_bytes();
    let link = header.link_name_bytes();
    let mode = header
        .mode()
        .map_err(|_| ContractError::verification("package PAX header has an invalid mode"))?;
    let uid = header
        .uid()
        .map_err(|_| ContractError::verification("package PAX header has an invalid owner"))?;
    let gid = header
        .gid()
        .map_err(|_| ContractError::verification("package PAX header has an invalid group"))?;
    let mtime = header
        .mtime()
        .map_err(|_| ContractError::verification("package PAX header has an invalid timestamp"))?;
    let size = header
        .size()
        .map_err(|_| ContractError::verification("package PAX header has an invalid size"))?;
    if path.as_ref() != b"././@PaxHeader"
        || link.as_ref().is_some_and(|value| !value.is_empty())
        || header.entry_type().as_byte() != b'x'
        || mode != 0
        || uid != 0
        || gid != 0
        || mtime != 0
        || size != expected_size
        || expected_size == 0
        || header.as_ustar().is_none()
    {
        return Err(ContractError::verification(
            "package tar PAX header does not use canonical writer metadata",
        ));
    }
    Ok(())
}

pub(super) fn account_pax_extension(total: &mut u64, size: u64) -> Result<(), ContractError> {
    if size == 0 || size > MAX_PAX_EXTENSION_BYTES {
        return Err(ContractError::verification(
            "package tar PAX extension exceeds the per-header resource limit",
        ));
    }
    *total = total
        .checked_add(size)
        .ok_or_else(|| ContractError::verification("package tar PAX byte count overflowed"))?;
    if *total > MAX_TOTAL_PAX_EXTENSION_BYTES {
        return Err(ContractError::verification(
            "package tar PAX extensions exceed the aggregate resource limit",
        ));
    }
    Ok(())
}

fn parse_pax_overrides(bytes: &[u8]) -> Result<PaxOverrides, ContractError> {
    let mut overrides = PaxOverrides::default();
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let separator = remaining
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| ContractError::verification("package tar PAX record is malformed"))?;
        let length_bytes = &remaining[..separator];
        if length_bytes.is_empty()
            || !length_bytes.iter().all(u8::is_ascii_digit)
            || (length_bytes.len() > 1 && length_bytes[0] == b'0')
        {
            return Err(ContractError::verification(
                "package tar PAX record has a noncanonical length",
            ));
        }
        let length_text = std::str::from_utf8(length_bytes)
            .map_err(|_| ContractError::verification("package tar PAX record is malformed"))?;
        let record_len = length_text
            .parse::<usize>()
            .map_err(|_| ContractError::verification("package tar PAX record length overflowed"))?;
        let end = offset.checked_add(record_len).ok_or_else(|| {
            ContractError::verification("package tar PAX record length overflowed")
        })?;
        if record_len <= separator + 3 || end > bytes.len() {
            return Err(ContractError::verification(
                "package tar PAX record length is inconsistent",
            ));
        }
        let record = &bytes[offset..end];
        if record.last() != Some(&b'\n') {
            return Err(ContractError::verification(
                "package tar PAX record is not newline terminated",
            ));
        }
        let key_value = &record[separator + 1..record.len() - 1];
        let equals = key_value
            .iter()
            .position(|byte| *byte == b'=')
            .ok_or_else(|| ContractError::verification("package tar PAX record is malformed"))?;
        let key = &key_value[..equals];
        let value = &key_value[equals + 1..];
        if value.contains(&0) || value.contains(&b'\\') {
            return Err(ContractError::verification(
                "package tar PAX path or linkpath is not canonical",
            ));
        }
        std::str::from_utf8(value).map_err(|_| {
            ContractError::verification("package tar PAX path or linkpath is not UTF-8")
        })?;
        match key {
            b"path" if !value.is_empty() && overrides.path.is_none() => {
                overrides.path = Some(value.to_vec());
            }
            b"linkpath" if !value.is_empty() && overrides.linkpath.is_none() => {
                overrides.linkpath = Some(value.to_vec());
            }
            b"path" | b"linkpath" => {
                return Err(ContractError::verification(
                    "package tar PAX path or linkpath is empty or duplicated",
                ));
            }
            _ => {
                return Err(ContractError::verification(
                    "package tar PAX extension overrides unsupported metadata",
                ));
            }
        }
        offset = end;
    }
    if overrides.path.is_none() && overrides.linkpath.is_none() {
        return Err(ContractError::verification(
            "package tar PAX extension contains no supported path metadata",
        ));
    }
    Ok(overrides)
}

fn validate_canonical_archive_path(
    bytes: &[u8],
    entry_type: u8,
    archive_root: &str,
) -> Result<(), ContractError> {
    let path = std::str::from_utf8(bytes)
        .map_err(|_| ContractError::verification("package tar entry path is not UTF-8"))?;
    let is_directory = entry_type == b'5';
    if path.is_empty()
        || path.starts_with('/')
        || path.bytes().any(|byte| byte == b'\\' || byte == 0)
        || path.contains("//")
        || (!is_directory && path.ends_with('/'))
    {
        return Err(ContractError::verification(
            "package tar entry path is not in canonical relative form",
        ));
    }
    let without_directory_slash = if is_directory && path.ends_with('/') {
        path.strip_suffix('/').unwrap_or(path)
    } else {
        path
    };
    if without_directory_slash
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
        || without_directory_slash
            .split('/')
            .next()
            .is_none_or(|root| root != archive_root)
    {
        return Err(ContractError::verification(
            "package tar entry path is not a canonical relative path",
        ));
    }
    Ok(())
}
