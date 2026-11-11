use crate::AppServerTarget;

/// Implicit daemon reuse must not bypass the interactive Code Mode runtime.
/// Explicit remote endpoints and the daemon-wide agents overview keep their
/// existing client capability contract.
pub(super) fn interactive_target(target: AppServerTarget, cli: &crate::Cli) -> AppServerTarget {
    if cli.agents_overview {
        return target;
    }
    match target {
        AppServerTarget::LocalDaemon { .. } | AppServerTarget::Embedded => {
            AppServerTarget::Embedded
        }
        remote @ (AppServerTarget::Remote { .. } | AppServerTarget::Cloud) => remote,
    }
}

#[cfg(test)]
#[path = "code_mode_startup_tests.rs"]
mod tests;
