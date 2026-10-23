//! Join retained native calls to the generic, independently selected Source reader.
//!
//! A genmodule producer roster is not the universe of compile owners. This
//! boundary uses the retained generated-Source input coordinate for each call;
//! the native owner projection independently verifies its actual reached goals.
//! It adds no execution, phase-equivalence or complete-selection authority.
//! This is an internal test prototype, not an active production API.

use crate::{
    make_vars::MakeProgramPaths,
    native_owner_projection::{MatchedChildSource, NativeOwnerProjection},
};
use aros_common::{
    local_source::LocalSourceIdentity, make_input_snapshot::MakeInputSnapshot,
    mesa_native_make::LoadedMesaNativeMake, mesa_source::MesaSourceSnapshot,
};
use std::path::Path;

/// Interpret one retained call using its original input/argv/environment.
///
/// `roots`, the native owner projection and the captured filesystem are caller
/// selections, not reconstructed from the packet. Source bytes must match the
/// retained source digest; measured source/build coordinates cannot be replaced
/// by `DirVars` aliases. A successful result contains the generic interpreted
/// program and source-positioned events, not copied compiler-option formulas.
///
/// This does not establish that all required calls were captured, that a
/// generated Makefile was produced from its corresponding Source input, or
/// that compilation/GCA hashing used this program. Those remain outer owning
/// transaction obligations. A caller using a local source identity must bind
/// that identity to its selected configuration input independently.
///
/// # Errors
/// Refuses absent calls, mismatched coordinates/source bytes, source-owner or
/// goal disagreement, unbound inputs, unknown semantics and input drift.
/// Errors do not include raw packet, environment or interpreter values.
pub fn read(
    native: &LoadedMesaNativeMake,
    call_index: usize,
    projection: &NativeOwnerProjection,
    roots: &[String],
    paths: &MakeProgramPaths<'_>,
    snapshot: &MakeInputSnapshot,
    local_source: Option<&LocalSourceIdentity>,
) -> Result<MatchedChildSource, String> {
    let call = native
        .binding()
        .source_calls
        .get(call_index)
        .ok_or("native Source call is absent")?;
    let coordinates = native.coordinates();
    if paths.source_root != Path::new(&coordinates.source_root)
        || paths.top_directory != Path::new(&coordinates.build_root)
    {
        return Err("native Source paths differ from configured coordinates".into());
    }
    if !snapshot.matches_root_selection(paths.top_directory, &native.binding().prepared_tree_sha256)
    {
        return Err("native Source input snapshot differs from retained preparation".into());
    }
    let observation = native
        .observation(call_index)
        .map_err(|_| "native Source observation is inconsistent")?
        .ok_or("native Source observation is absent")?;
    let source = MesaSourceSnapshot::capture(
        paths.source_root,
        &native.binding().source_inputs_sha256,
        local_source,
    )
    .map_err(|_| "native Source bytes differ from retained selection")?;
    let result = projection
        .read_retained_child_source(paths, roots, &call.generated_owner, &observation, snapshot)
        .map_err(|_| "native Source program interpretation failed")?;
    source
        .verify(paths.source_root)
        .map_err(|_| "native Source changed during interpretation")?;
    Ok(result)
}
