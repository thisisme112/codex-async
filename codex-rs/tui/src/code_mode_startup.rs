use crate::AppServerTarget;

/// Implicit daemon reuse must not bypass the interactive Code Mode runtime.
/// Explicit remote endpoints and the daemon-wide agents overview keep their
/// existing client capability contract.
pub(super) fn interactive_target(target: AppServerTarget, cli: &crate::Cli) -> AppServerTarget {
    if !cli.agents_overview
        && matches!(
            target,
            AppServerTarget::LocalDaemon {
                allow_embedded_fallback: true,
                ..
            }
        )
    {
        AppServerTarget::Embedded
    } else {
        target
    }
}

#[cfg(test)]
#[path = "code_mode_startup_tests.rs"]
mod tests;
