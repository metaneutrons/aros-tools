use super::{cmake_arg, cmake_literal_arg};
use crate::graph::DependencyGraph;
use std::fmt::Write;

/// Emits define headers, bison outputs and directory setups.
pub(super) fn emit_define_and_setup_rules(out: &mut String, graph: &DependencyGraph) {
    // Declaration-owned literal define headers. Concrete compile targets have
    // already been declared, so the helper can attach direct dependencies and
    // the output directory as a private include path without deferred target
    // lookup. The owner is a real output target, never a configure-time phony.
    if !graph.define_headers.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Generated headers from literal define fragments\n\
             # ============================================================================="
        )
        .unwrap();
        let mut headers: Vec<_> = graph.define_headers.iter().collect();
        headers.sort_by(|left, right| {
            left.owner
                .cmp(&right.owner)
                .then_with(|| left.file.cmp(&right.file))
                .then_with(|| left.line.cmp(&right.line))
        });
        for header in headers {
            writeln!(out, "aros_generate_defines_header(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&header.owner)).unwrap();
            writeln!(out, "    OUTPUT {}", cmake_arg(&header.output)).unwrap();
            let definitions: Vec<_> = header
                .definitions
                .iter()
                .map(|definition| cmake_arg(definition))
                .collect();
            writeln!(out, "    DEFINES {}", definitions.join(" ")).unwrap();
            if !header.dependencies.is_empty() {
                let dependencies: Vec<_> = header
                    .dependencies
                    .iter()
                    .map(|dependency| cmake_arg(dependency))
                    .collect();
                writeln!(out, "    DEPENDS {}", dependencies.join(" ")).unwrap();
            }
            if !header.consumers.is_empty() {
                let consumers: Vec<_> = header
                    .consumers
                    .iter()
                    .map(|consumer| cmake_arg(consumer))
                    .collect();
                writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    if !graph.bison_outputs.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Generated C sources from exact host-Bison recipes\n\
             # ============================================================================="
        )
        .unwrap();
        let mut outputs: Vec<_> = graph.bison_outputs.iter().collect();
        outputs.sort_by(|left, right| left.output.cmp(&right.output));
        for declaration in outputs {
            writeln!(out, "aros_generate_bison_output(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(out, "    INPUT {}", cmake_arg(&declaration.input)).unwrap();
            writeln!(out, "    OUTPUT {}", cmake_arg(&declaration.output)).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    for declaration in &graph.directory_setups {
        writeln!(out, "aros_prepare_directories(").unwrap();
        writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
        let directories = declaration
            .directories
            .iter()
            .map(|directory| cmake_arg(directory))
            .collect::<Vec<_>>();
        writeln!(out, "    DIRECTORIES {}", directories.join(" ")).unwrap();
        writeln!(out, ")\n").unwrap();
    }
}

/// Emits genmodule, SFD and SDK text rules.
pub(super) fn emit_genmodule_and_text_rules(out: &mut String, graph: &DependencyGraph) {
    for declaration in &graph.genmodule_header_rules {
        writeln!(out, "aros_genmodule_header_stamp(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("LAYOUT", declaration.layout.as_str()),
            ("DECLARING_DIR", declaration.declaring_dir.as_str()),
            ("CONFIG", declaration.config.as_str()),
            ("MODULE", declaration.module.as_str()),
            ("MODTYPE", declaration.modtype.as_str()),
            ("INCLUDE_NAME", declaration.include_name.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.genmodule_writefiles_rules {
        writeln!(out, "aros_genmodule_writefiles_stamp(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("CONFIG", declaration.config.as_str()),
            ("MODULE", declaration.module.as_str()),
            ("MODTYPE", declaration.modtype.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.sfd_header_rules {
        writeln!(out, "aros_generate_sfd_headers(").unwrap();
        writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
        writeln!(out, "    FILE {}", cmake_arg(&declaration.file)).unwrap();
        writeln!(
            out,
            "    FILE_SHA256 {}",
            cmake_arg(&declaration.file_sha256)
        )
        .unwrap();
        writeln!(out, "    JOBS").unwrap();
        for job in &declaration.jobs {
            let encoded = format!(
                "{}|{}|{}|{}|{}",
                job.mode, job.target, job.input, job.input_sha256, job.sdk_output
            );
            writeln!(out, "        {}", cmake_arg(&encoded)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.sdk_text_rules {
        writeln!(out, "aros_transform_sdk_text(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("INPUT", declaration.input.as_str()),
            ("OUTPUT", declaration.output.as_str()),
            ("FETCH", declaration.fetch_owner.as_str()),
            ("FILE", declaration.file.as_str()),
            ("FILE_SHA256", declaration.file_sha256.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        writeln!(out, "    OPERATIONS").unwrap();
        for operation in &declaration.operations {
            writeln!(
                out,
                "        {}",
                cmake_literal_arg(&operation.cmake_argument())
            )
            .unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }
}

/// Emits source text/value rules and SDK file copies.
pub(super) fn emit_source_text_and_copy_rules(out: &mut String, graph: &DependencyGraph) {
    for declaration in &graph.source_text_rules {
        for product in &declaration.outputs {
            writeln!(out, "aros_transform_source_text(").unwrap();
            for (key, value) in [
                ("NAME", declaration.owner.as_str()),
                ("INPUT", product.input.as_str()),
                ("OUTPUT", product.output.as_str()),
                ("FETCH", declaration.fetch_owner.as_str()),
            ] {
                writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
            }
            let operations = serde_json::to_string(&product.operations)
                .expect("closed source text operations serialize as JSON");
            // The closed scanner reserves this deferred root for source Make
            // directory expressions. Other dollars (for example pkg-config
            // ${prefix}) remain literal; never expose arbitrary CMake names.
            let operations_argument = cmake_literal_arg(&operations)
                .replace("\\${AROS_BUILD_DIR}", "${AROS_BUILD_DIR}")
                .replace("\\${AROS_SDK_INCLUDE_DIR}", "${AROS_SDK_INCLUDE_DIR}")
                .replace("\\${AROS_GENINC_DIR}", "${AROS_GENINC_DIR}");
            writeln!(out, "    OPERATIONS_JSON {operations_argument}").unwrap();
            if let Some(mode) = &product.mode {
                writeln!(out, "    MODE {}", cmake_arg(mode)).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    for declaration in &graph.source_value_rules {
        writeln!(out, "aros_extract_source_value(").unwrap();
        let mut marker_hex = String::with_capacity(declaration.marker.len() * 2);
        for byte in declaration.marker.as_bytes() {
            write!(marker_hex, "{byte:02x}").unwrap();
        }
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("INPUT", declaration.input.as_str()),
            ("OUTPUT", declaration.output.as_str()),
            ("MARKER_HEX", marker_hex.as_str()),
            ("FILE", declaration.file.as_str()),
            ("FILE_SHA256", declaration.file_sha256.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }
    for declaration in &graph.sdk_file_copies {
        writeln!(out, "aros_stage_sdk_files(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("SOURCE", declaration.source_dir.as_str()),
            ("DESTINATION", declaration.destination.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        if let Some(fetch) = &declaration.fetch_owner {
            writeln!(out, "    FETCH {}", cmake_arg(fetch)).unwrap();
        }
        writeln!(out, "    FILES").unwrap();
        for file in &declaration.files {
            writeln!(out, "        {}", cmake_literal_arg(file)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }
}

/// Emits SDK asset rules in dependency order.
pub(super) fn emit_sdk_asset_rules(out: &mut String, graph: &DependencyGraph) {
    let mut pending_assets: Vec<_> = graph.sdk_asset_rules.iter().collect();
    pending_assets.sort_by(|a, b| a.owner.cmp(&b.owner));
    let asset_owners: std::collections::BTreeSet<_> = pending_assets
        .iter()
        .map(|rule| rule.owner.as_str())
        .collect();
    let mut emitted_assets = std::collections::BTreeSet::new();
    while !pending_assets.is_empty() {
        let next = pending_assets.iter().position(|rule| {
            graph
                .sdk_asset_dependencies(&rule.owner, None)
                .is_ok_and(|dependencies| {
                    dependencies.iter().all(|owner| {
                        !asset_owners.contains(owner.as_str()) || emitted_assets.contains(owner)
                    })
                })
        });
        let Some(index) = next else {
            writeln!(out, "message(FATAL_ERROR \"SDK assets require unique concrete input producers and an acyclic file graph\")").unwrap();
            break;
        };
        let declaration = pending_assets.remove(index);
        for operation in &declaration.operations {
            writeln!(out, "aros_sdk_asset_rule(").unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
            match &operation.operation {
                crate::sdk_asset_rules::SdkAssetOperation::Copy { input, output } => {
                    let producers = graph.sdk_asset_producers(None);
                    let producer = producers[&input.to_ascii_lowercase()]
                        .iter()
                        .next()
                        .unwrap();
                    writeln!(out, "    INPUT {}", cmake_arg(input)).unwrap();
                    writeln!(out, "    OUTPUT {}", cmake_arg(output)).unwrap();
                    writeln!(out, "    PRODUCER {}", cmake_arg(producer)).unwrap();
                }
                crate::sdk_asset_rules::SdkAssetOperation::WriteText { output, text } => {
                    let mut hex = String::new();
                    for byte in text.as_bytes() {
                        write!(hex, "{byte:02x}").unwrap();
                    }
                    writeln!(out, "    OUTPUT {}", cmake_arg(output)).unwrap();
                    writeln!(out, "    TEXT_HEX {}", cmake_literal_arg(&hex)).unwrap();
                }
            }
            writeln!(out, ")\n").unwrap();
        }
        emitted_assets.insert(declaration.owner.clone());
    }
}

/// Emits host header aggregates and rules.
pub(super) fn emit_host_header_rules(out: &mut String, graph: &DependencyGraph) {
    for declaration in &graph.host_header_aggregates {
        writeln!(out, "aros_host_header_aggregate(").unwrap();
        writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
        writeln!(out, "    TOOL {}", cmake_arg(&declaration.tool)).unwrap();
        writeln!(
            out,
            "    SOURCE {}",
            cmake_arg(&format!("${{AROS_SOURCE_DIR}}/{}", declaration.tool_source))
        )
        .unwrap();
        if !declaration.host_compile_flags.is_empty() {
            writeln!(
                out,
                "    COMPILE_FLAGS {}",
                declaration
                    .host_compile_flags
                    .iter()
                    .map(|flag| cmake_literal_arg(flag))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .unwrap();
        }
        writeln!(out, ")\n").unwrap();
        for header in &declaration.headers {
            writeln!(out, "aros_host_header_aggregate_output(").unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(out, "    HEADER {}", cmake_literal_arg(&header.header)).unwrap();
            if header.generated_mirror {
                writeln!(out, "    GENERATED_MIRROR").unwrap();
            }
            if !header.arguments.is_empty() {
                writeln!(
                    out,
                    "    ARGUMENTS {}",
                    header
                        .arguments
                        .iter()
                        .map(|argument| cmake_literal_arg(argument))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
                .unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    for declaration in &graph.host_header_rules {
        writeln!(out, "aros_host_header_rule(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("SETUP_NAME", declaration.setup_owner.as_str()),
            ("TOOL_SOURCE", declaration.tool_source.as_str()),
            ("TOOL_OUTPUT", declaration.tool_output.as_str()),
            ("SOURCE_WORKDIR", declaration.source_workdir.as_str()),
            ("HEADER", declaration.header.as_str()),
            ("PRIMARY_OUTPUT", declaration.primary_output.as_str()),
            ("SDK_OUTPUT", declaration.sdk_output.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        if declaration.use_configured_host_cflags {
            writeln!(out, "    USE_CONFIGURED_HOST_CFLAGS").unwrap();
        }
        for (key, values) in [
            ("COMPILE_FLAGS", &declaration.host_compile_flags),
            ("SOURCE_PREREQUISITES", &declaration.source_prerequisites),
            ("SETUP_DIRECTORIES", &declaration.setup_directories),
        ] {
            if !values.is_empty() {
                let values = values
                    .iter()
                    .map(|value| cmake_arg(value))
                    .collect::<Vec<_>>();
                writeln!(out, "    {key} {}", values.join(" ")).unwrap();
            }
        }
        writeln!(out, ")\n").unwrap();
    }
}

/// Emits host file generators.
pub(super) fn emit_host_file_generators(out: &mut String, graph: &DependencyGraph) {
    for declaration in &graph.host_file_generators {
        let manifest = serde_json::to_string(&declaration.inputs.iter().map(|input| {
            serde_json::json!({"filename": input.filename, "sha256": input.sha256, "size": input.size})
        }).collect::<Vec<_>>()).expect("host inputs serialize");
        let source_manifest = serde_json::to_string(
            &graph
                .host_file_generator_source_digests
                .iter()
                .map(|(path, sha256)| serde_json::json!({"path": path, "sha256": sha256}))
                .collect::<Vec<_>>(),
        )
        .expect("source inputs serialize");
        let tool_sha = graph
            .host_file_generator_source_digests
            .get(&declaration.tool_source)
            .expect("admitted source-owned host tool must have its sealed digest");
        writeln!(out, "aros_host_c_file_generator(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.clone()),
            (
                "TOOL_SOURCE",
                format!("${{AROS_SOURCE_DIR}}/{}", declaration.tool_source),
            ),
            ("TOOL_SHA256", tool_sha.clone()),
            (
                "OUTPUT",
                format!("${{AROS_BUILD_DIR}}/{}", declaration.output),
            ),
            (
                "INPUT_DIRECTORY",
                "${AROS_NATIVE_HOST_INPUT_DIRECTORY}".into(),
            ),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(&value)).unwrap();
        }
        writeln!(
            out,
            "    INPUT_MANIFEST_JSON {}",
            cmake_literal_arg(&manifest)
        )
        .unwrap();
        writeln!(
            out,
            "    SOURCE_MANIFEST_JSON {}",
            cmake_literal_arg(&source_manifest)
        )
        .unwrap();
        for (key, values) in [
            ("COMPILE_FLAGS", &declaration.compile_flags),
            ("ARGUMENTS", &declaration.arguments),
        ] {
            writeln!(
                out,
                "    {key} {}",
                values
                    .iter()
                    .map(|value| cmake_literal_arg(value))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }
}

/// Emits header transforms.
pub(super) fn emit_header_transforms(out: &mut String, graph: &DependencyGraph) {
    // Safe hand-written header transforms.  Concrete consumers have already
    // been declared, while fetch targets were emitted first, so CMake can bind
    // both sides directly without deferred target-name guessing.
    if !graph.header_transforms.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Generated headers from safe literal transforms\n\
             # ============================================================================="
        )
        .unwrap();
        let mut transforms: Vec<_> = graph.header_transforms.iter().collect();
        transforms.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.file.cmp(&right.file))
                .then_with(|| left.line.cmp(&right.line))
        });
        for transform in transforms {
            writeln!(out, "aros_transform_header(").unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&transform.name)).unwrap();
            writeln!(out, "    INPUT {}", cmake_arg(&transform.input)).unwrap();
            writeln!(out, "    OUTPUT {}", cmake_arg(&transform.output)).unwrap();
            if let Some(owner) = &transform.generated_input_owner {
                writeln!(out, "    GENERATED_INPUT_OWNER {}", cmake_arg(owner)).unwrap();
            }
            if transform.copy_only {
                writeln!(out, "    COPY_ONLY").unwrap();
            } else if !transform.substitutions.is_empty() {
                let substitutions: Vec<_> = transform
                    .substitutions
                    .iter()
                    .map(|value| cmake_arg(value))
                    .collect();
                writeln!(out, "    SUBSTITUTIONS {}", substitutions.join(" ")).unwrap();
            } else {
                if transform.replace_whole_line_containing {
                    writeln!(out, "    WHOLE_LINE_CONTAINING").unwrap();
                }
                writeln!(out, "    MATCH {}", cmake_arg(&transform.match_text)).unwrap();
                writeln!(
                    out,
                    "    REPLACEMENT {}",
                    cmake_literal_arg(&transform.replacement)
                )
                .unwrap();
            }
            if !transform.dependencies.is_empty() {
                let dependencies: Vec<_> = transform
                    .dependencies
                    .iter()
                    .map(|dependency| cmake_arg(dependency))
                    .collect();
                writeln!(out, "    DEPENDS {}", dependencies.join(" ")).unwrap();
            }
            if !transform.consumers.is_empty() {
                let consumers: Vec<_> = transform
                    .consumers
                    .iter()
                    .map(|consumer| cmake_arg(consumer))
                    .collect();
                writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }
}

/// Emits icon targets.
pub(super) fn emit_icon_targets(out: &mut String, graph: &DependencyGraph) {
    // Workbench .info resources. Identities are declared separately from
    // their output rules so an unresolved or architecture-empty declaration
    // remains a real, nameable target and keeps its #MM edges.
    if !graph.icon_targets.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Workbench icons (from %build_icons)\n\
             # ============================================================================="
        )
        .unwrap();

        let mut icon_targets: Vec<_> = graph.icon_targets.values().collect();
        icon_targets.sort_by(|a, b| a.mmake.cmp(&b.mmake));
        for target in icon_targets {
            writeln!(out, "aros_declare_icon_target(").unwrap();
            // Deliberately unquoted: aros-verify reads the token as written.
            writeln!(out, "    MMAKE_ID {}", target.mmake).unwrap();
            writeln!(out, "    DIRECTORY {}", cmake_arg(&target.directory)).unwrap();
            writeln!(out, ")").unwrap();
        }
        writeln!(out).unwrap();

        let mut icons: Vec<_> = graph.icons.iter().collect();
        icons.sort_by(|a, b| {
            a.srcdir
                .cmp(&b.srcdir)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.dir.cmp(&b.dir))
                .then_with(|| a.mmake.cmp(&b.mmake))
                .then_with(|| a.condition.cmp(&b.condition))
        });
        for icon in icons {
            if let Some(condition) = &icon.condition {
                writeln!(out, "if({condition})").unwrap();
            }
            writeln!(out, "aros_build_icons(").unwrap();
            writeln!(out, "    MMAKE_ID {}", icon.mmake).unwrap();
            writeln!(out, "    DIRECTORY {}", cmake_arg(&icon.srcdir)).unwrap();
            writeln!(out, "    DESTINATION {}", cmake_arg(&icon.dir)).unwrap();
            writeln!(out, "    FORMAT {}", cmake_arg(&icon.fmt)).unwrap();
            if let Some(iconset) = &icon.iconset {
                writeln!(out, "    ICONSET {}", cmake_arg(iconset)).unwrap();
            }
            if !icon.icons.is_empty() {
                let values: Vec<_> = icon.icons.iter().map(|v| cmake_arg(v)).collect();
                writeln!(out, "    ICONS {}", values.join(" ")).unwrap();
            }
            if !icon.images.is_empty() {
                let values: Vec<_> = icon.images.iter().map(|v| cmake_arg(v)).collect();
                writeln!(out, "    IMAGES {}", values.join(" ")).unwrap();
            }
            writeln!(out, ")").unwrap();
            if icon.condition.is_some() {
                writeln!(out, "endif()").unwrap();
            }
            writeln!(out).unwrap();
        }
    }
}

/// Emits message catalogs.
pub(super) fn emit_catalogs(out: &mut String, graph: &DependencyGraph) {
    // Translated Locale catalogs. Each declaration owns every requested
    // `.catalog` plus its optional generated source/header; unresolved
    // declarations are deliberately absent and remain visible in the skip and
    // coverage reports rather than becoming phony stubs.
    if !graph.catalogs.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Locale catalogs (from %build_catalogs)\n\
             # ============================================================================="
        )
        .unwrap();

        let mut catalogs: Vec<_> = graph.catalogs.iter().collect();
        catalogs.sort_by(|a, b| {
            a.mmake
                .cmp(&b.mmake)
                .then_with(|| a.declaring_dir.cmp(&b.declaring_dir))
                .then_with(|| a.line.cmp(&b.line))
        });
        for catalog in catalogs {
            writeln!(out, "aros_build_catalogs(").unwrap();
            // Deliberately unquoted: aros-verify reads this token as written.
            writeln!(out, "    MMAKE_ID {}", catalog.mmake).unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&catalog.name)).unwrap();
            writeln!(out, "    SUBDIR {}", cmake_arg(&catalog.subdir)).unwrap();
            writeln!(out, "    DIRECTORY {}", cmake_arg(&catalog.declaring_dir)).unwrap();
            writeln!(out, "    SOURCE_DIR {}", cmake_arg(&catalog.srcdir)).unwrap();
            writeln!(out, "    DESTINATION {}", cmake_arg(&catalog.dir)).unwrap();
            writeln!(out, "    DESCRIPTION {}", cmake_arg(&catalog.description)).unwrap();
            if let Some(source) = &catalog.source {
                writeln!(out, "    SOURCE {}", cmake_arg(source)).unwrap();
            }
            if !catalog.consumers.is_empty() {
                let consumers: Vec<_> = catalog
                    .consumers
                    .iter()
                    .map(|consumer| cmake_arg(consumer))
                    .collect();
                writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            }
            writeln!(
                out,
                "    SOURCE_DESCRIPTION {}",
                cmake_arg(&catalog.source_description)
            )
            .unwrap();
            let languages: Vec<_> = catalog
                .catalogs
                .iter()
                .map(|language| cmake_arg(language))
                .collect();
            writeln!(out, "    LANGUAGES {}", languages.join(" ")).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }
}
