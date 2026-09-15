/// Whether this execution's provider acknowledged non-consuming completion events.
///
/// This is a runtime capability, not a promise to start a model turn. A caller
/// must separately track yielded output exposure, result retrieval and cleanup.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompletionReadySupport {
    #[default]
    Unsupported,
    Supported,
}

#[cfg(test)]
#[path = "completion_ready_tests.rs"]
mod tests;
