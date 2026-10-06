//! Parser-owned vocabulary for managed compiler-cache policy.

use clap::ValueEnum;

/// Compiler-cache policy shared by product, board and producer builds.
#[derive(Clone, Copy, ValueEnum)]
pub enum BuildCompilerCache {
    /// Use prepared local backends in order (sccache, then ccache); works offline.
    Auto,
    /// Do not use a compiler-cache launcher.
    Off,
    /// Require a prepared AROS-owned local namespace; works offline.
    Sccache,
    /// Require a prepared AROS-owned local namespace; works offline.
    Ccache,
}
