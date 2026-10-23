//! Checked Source export differences; engine and shell effects stay distinct.
use super::MakeRecipeProjection;
use aros_common::make_native_invocation::NativeMakeInvocation;
use std::collections::BTreeMap;

impl MakeRecipeProjection {
    /// Restore only changed Source-controlled exports from an independently
    /// held original projection. Every changed instrumented value (including
    /// absence) must first match the observed process exactly.
    ///
    /// Unchanged exports and opaque process keys are preserved byte-for-byte.
    /// In particular this does NOT validate or reconstruct Make's engine-owned
    /// environment or shell process effects. It is data, not launch authority
    /// or a complete original-environment equivalence proof. The caller applies
    /// later literal assignments before bounding its FINAL child invocation;
    /// this source-bounded map is not itself a native execution channel.
    /// # Errors
    /// Refuses a changed export's mismatching value or presence.
    pub fn apply_changed_source_exports(
        &self,
        instrumented: &Self,
        observed: &NativeMakeInvocation,
    ) -> Result<BTreeMap<String, String>, String> {
        apply_delta(
            self.source_exports(),
            instrumented.source_exports(),
            observed,
        )
    }
}

fn apply_delta(
    original: &BTreeMap<String, String>,
    instrumented: &BTreeMap<String, String>,
    observed: &NativeMakeInvocation,
) -> Result<BTreeMap<String, String>, String> {
    let mut environment = observed.environment().clone();
    for (name, value) in instrumented {
        if original.get(name) == Some(value) {
            continue;
        }
        if observed.environment().get(name) != Some(value) {
            return Err("changed Source export differs from observed process data".into());
        }
        if let Some(original) = original.get(name) {
            environment.insert(name.clone(), original.clone());
        } else {
            environment.remove(name);
        }
    }
    for (name, value) in original {
        if !instrumented.contains_key(name) {
            if observed.environment().contains_key(name) {
                return Err(
                    "original-only Source export conflicts with observed process data".into(),
                );
            }
            environment.insert(name.clone(), value.clone());
        }
    }
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect()
    }

    fn observed(entries: &[(&str, &str)]) -> NativeMakeInvocation {
        NativeMakeInvocation::new(["literal-arg".into()], "/captured".into(), map(entries))
            .unwrap()
            .with_argument_zero("/selected/program".into())
            .unwrap()
    }

    #[test]
    fn source_delta_preserves_opaque_and_engine_values_without_mutating_raw_data() {
        let original = map(&[("ROLE", "original"), ("ADDED", ""), ("MAKELEVEL", "0")]);
        let instrumented = map(&[
            ("ROLE", "recorder"),
            ("REMOVED", "instrumented-only"),
            ("MAKELEVEL", "0"),
        ]);
        let raw = observed(&[
            ("ROLE", "recorder"),
            ("REMOVED", "instrumented-only"),
            ("HAS-DASH", "opaque$raw"),
            ("MAKELEVEL", "1"),
            ("_", "/observer"),
        ]);
        let restored = apply_delta(&original, &instrumented, &raw).unwrap();
        assert_eq!(
            restored,
            map(&[
                ("ROLE", "original"),
                ("ADDED", ""),
                ("HAS-DASH", "opaque$raw"),
                ("MAKELEVEL", "1"),
                ("_", "/observer")
            ])
        );
        assert_eq!(raw.arguments(), ["literal-arg"]);
        assert_eq!(raw.argument_zero(), Some("/selected/program"));
        assert_eq!(raw.environment()["ROLE"], "recorder");
    }

    #[test]
    fn source_delta_refuses_wrong_missing_or_unexpected_changed_values() {
        for entries in [
            vec![("ROLE", "wrong")],
            vec![],
            vec![("ROLE", ""), ("ADDED", "unexpected")],
        ] {
            let raw = observed(&entries);
            let before = raw.to_bytes().unwrap();
            let result = apply_delta(
                &map(&[("ROLE", "original"), ("ADDED", "")]),
                &map(&[("ROLE", "")]),
                &raw,
            );
            assert!(result.is_err());
            assert_eq!(raw.to_bytes().unwrap(), before);
        }
        assert!(apply_delta(&BTreeMap::new(), &map(&[("REMOVED", "")]), &observed(&[])).is_err());
    }

    #[test]
    fn source_delta_leaves_final_prefix_and_native_bounds_to_the_child_boundary() {
        let original = BTreeMap::from([("LARGE".into(), "x".repeat(128 * 1024))]);
        let restored = apply_delta(&original, &BTreeMap::new(), &observed(&[])).unwrap();
        assert!(NativeMakeInvocation::new([], "/captured".into(), restored.clone()).is_err());
        let mut prefixed = restored;
        prefixed.insert("LARGE".into(), "small final value".into());
        assert!(NativeMakeInvocation::new([], "/captured".into(), prefixed).is_ok());
    }
}
