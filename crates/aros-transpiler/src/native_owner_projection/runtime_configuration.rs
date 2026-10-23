use crate::metamake_project::{ConfiguredProject, ProjectGlobalPathContext};
use std::collections::{BTreeMap, BTreeSet};

/// A scoped comparison against the runtime configuration captured from Source.
///
/// Instances are created only by `NativeOwnerProjection` while it verifies a
/// complete runtime configuration batch.
pub struct RuntimeConfigurationCheck<'a> {
    expected_project: &'a ConfiguredProject,
    expected_globals: &'a BTreeMap<String, String>,
    absent_globals: &'a BTreeSet<String>,
}

impl<'a> RuntimeConfigurationCheck<'a> {
    pub(super) const fn new(
        expected_project: &'a ConfiguredProject,
        expected_globals: &'a BTreeMap<String, String>,
        absent_globals: &'a BTreeSet<String>,
    ) -> Self {
        Self {
            expected_project,
            expected_globals,
            absent_globals,
        }
    }

    /// Compare project metadata and the selected present/absent global values.
    ///
    /// Command execution and complete environment qualification remain outside
    /// this metadata comparison.
    ///
    /// # Errors
    /// Refuses different project metadata or selected global values/absences.
    pub fn compare(
        &self,
        project: &ConfiguredProject,
        globals: &ProjectGlobalPathContext,
    ) -> Result<(), String> {
        let expected = self.expected_project;
        if project.name != expected.name
            || project.top != expected.top
            || project.default_makefile != expected.default_makefile
            || project.default_target != expected.default_target
            || project.ignored_directories != expected.ignored_directories
            || project.added_makefiles != expected.added_makefiles
            || project.global_variable_files != expected.global_variable_files
            || project.generator_inputs != expected.generator_inputs
            || project.generator_command != expected.generator_command
            || project.global_generator_command != expected.global_generator_command
            || project.variables != expected.variables
        {
            return Err("runtime owner project metadata differs from Source policy".into());
        }
        globals.verify_selected_values(self.expected_globals, self.absent_globals)
    }
}
