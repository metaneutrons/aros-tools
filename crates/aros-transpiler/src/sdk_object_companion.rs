//! Explicit source-bound compiler contexts for SDK companion objects.
use super::{
    evaluate_make_list, parse_compile_flags, safe_target_name, BTreeMap, CompileFlags, DirVars,
    MakeExprContext, Path, VarScope,
};

pub(super) fn companion_compile_role(
    arguments: &BTreeMap<String, String>,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Result<Option<(String, CompileFlags)>, String> {
    let Some(cflags) = arguments.get("cflags") else {
        return Ok(None);
    };
    if cflags != "$(strip $(TARGET_ISA_32_CFLAGS) $(TARGET_CFLAGS))" {
        return Err("SDK companion requires the exact source compiler-context expression".into());
    }
    let namespace = arguments
        .get("mmake")
        .ok_or("SDK companion requires an explicit compiler namespace")?;
    if !safe_target_name(namespace) {
        return Err("SDK companion compiler namespace is unsafe".into());
    }
    let [role] = dirs.sdk_companion_roles() else {
        return Err("SDK companion requires one explicitly admitted source ABI role".into());
    };
    if scope.raw_at("TARGET_ISA_32_CFLAGS", line).as_deref() != Some("$(NATIVE_TARGET_32_CFLAGS)") {
        return Err("SDK companion ISA flags are not bound to the admitted source ABI".into());
    }
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let isa = evaluate_make_list("$(TARGET_ISA_32_CFLAGS)", &context)
        .map_err(|error| error.to_string())?;
    if isa != role.compiler_options {
        return Err("SDK companion compiler flags differ from the admitted ABI".into());
    }
    let base =
        evaluate_make_list("$(TARGET_CFLAGS)", &context).map_err(|error| error.to_string())?;
    let flags = parse_compile_flags(&base, scope, dirs, root, rel_dir, line)?;
    Ok(Some((role.selector_cpu.clone(), flags)))
}
