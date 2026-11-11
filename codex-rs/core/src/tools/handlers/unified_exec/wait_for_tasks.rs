use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::PostToolUsePayload;
use crate::tools::registry::PreToolUsePayload;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use std::collections::BTreeMap;

pub struct WaitForTasksHandler;

#[derive(Deserialize)]
struct WaitForTasksArgs {
    session_ids: Vec<i32>,
}

impl ToolExecutor<ToolInvocation> for WaitForTasksHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("wait_for_tasks")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: "wait_for_tasks".to_string(),
            description: "Suspend this tool call until ALL listed tracked background commands finish. Waits on completion events, without polling or intermediate model requests; user interruption cancels the wait without terminating commands. Use only session IDs whose completion_notification_registered is true. To do other work, simply end the turn instead and receive automatic completion notification. On return, use write_stdin once per completed session to retrieve output and exit status. Do not use for servers or commands awaiting input.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([("session_ids".to_string(), JsonSchema::array(
                    JsonSchema::number(Some("Tracked unified exec session ID.".to_string())),
                    Some("1 to 64 distinct session IDs; waits until all finish.".to_string()),
                ))]),
                Some(vec!["session_ids".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "wait_for_tasks expects JSON arguments".to_string(),
                ));
            };
            let args: WaitForTasksArgs = parse_arguments(arguments)?;
            invocation
                .session
                .services
                .code_mode_service
                .completions
                .wait_for_commands(&args.session_ids, &invocation.cancellation_token)
                .await
                .map_err(FunctionCallError::RespondToModel)?;
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                serde_json::json!({"completed_session_ids":args.session_ids,"output_consumed":false}).to_string(),
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for WaitForTasksHandler {
    fn pre_tool_use_payload(&self, _invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        None
    }
    fn post_tool_use_payload(
        &self,
        _invocation: &ToolInvocation,
        _result: &dyn crate::tools::context::ToolOutput,
    ) -> Option<PostToolUsePayload> {
        None
    }
}
