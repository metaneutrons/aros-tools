//! Registry-backed CLI parsing for stable board model identities.

use aros_board::config::BoardId;
use aros_common::board_registry::BoardRegistry;
use clap::{
    builder::{PossibleValue, TypedValueParser},
    error::{ContextKind, ContextValue, ErrorKind},
};
use std::ffi::OsStr;
use std::sync::OnceLock;

/// Clap parser that accepts only IDs from the embedded board registry.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoardIdValueParser;

/// Return the cached embedded board catalog, retaining its first validation error.
pub fn embedded_board_registry() -> std::result::Result<&'static BoardRegistry, &'static str> {
    static REGISTRY: OnceLock<std::result::Result<BoardRegistry, String>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            aros_common::board_registry::built_in_board_registry()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(String::as_str)
}

impl TypedValueParser for BoardIdValueParser {
    type Value = BoardId;

    fn parse_ref(
        &self,
        command: &clap::Command,
        argument: Option<&clap::Arg>,
        value: &OsStr,
    ) -> std::result::Result<Self::Value, clap::Error> {
        let raw = value.to_string_lossy().into_owned();
        let registry = embedded_board_registry().map_err(|error| {
            clap::Error::raw(
                ErrorKind::ValueValidation,
                format!("the embedded board catalog is invalid: {error}"),
            )
            .with_cmd(command)
        })?;
        registry.get(&raw).map_or_else(
            |_| {
                let argument_name = argument.map_or_else(|| "...".to_owned(), ToString::to_string);
                let valid_values = registry
                    .boards()
                    .map(|contract| contract.id().as_str().to_owned())
                    .collect::<Vec<_>>();
                let mut error = clap::Error::new(ErrorKind::InvalidValue).with_cmd(command);
                error.insert(ContextKind::InvalidArg, ContextValue::String(argument_name));
                error.insert(ContextKind::InvalidValue, ContextValue::String(raw));
                error.insert(ContextKind::ValidValue, ContextValue::Strings(valid_values));
                Err(error)
            },
            |contract| Ok(contract.id().clone()),
        )
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let registry = embedded_board_registry().ok()?;
        Some(Box::new(
            registry
                .boards()
                .map(|contract| PossibleValue::new(contract.id().as_str())),
        ))
    }
}
