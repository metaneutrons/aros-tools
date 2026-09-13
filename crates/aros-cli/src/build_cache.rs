//! Parser-owned vocabulary for the product and board compiler-cache option.

use clap::ValueEnum;

/// Explicit compiler-cache launcher policy shared by all CMake build commands.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum BuildCompilerCache {
    /// Use sccache first, then ccache; offline builds disable automatic caching.
    Auto,
    /// Do not configure a compiler-cache launcher.
    Off,
    /// Require sccache; an offline build rejects an unverified storage scope.
    Sccache,
    /// Require ccache; an offline build rejects an unverified storage scope.
    Ccache,
}
