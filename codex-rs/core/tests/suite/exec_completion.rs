//! Local interactive command wake through the same admission path as Cells.
use codex_core::TurnInputRequest;
use codex_features::Feature;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::sse;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;
use test_case::test_case;
use wiremock::MockServer;

async fn build(server: &MockServer, nested: bool, enabled: bool) -> anyhow::Result<TestCodex> {
    test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_config(move |config| {
            config
                .features
                .enable(Feature::UnifiedExec)
                .expect("enable unified exec");
            if nested {
                config
                    .features
                    .enable(Feature::CodeMode)
                    .expect("enable Code Mode");
            }
        })
        .with_thread_manager(move |manager| {
            if enabled {
                manager.enable_code_mode_completion_wake();
            }
            manager
        })
        .build_with_auto_env(server)
        .await
}
fn user() -> TurnInputRequest {
    TurnInputRequest::user_input(vec![UserInput::Text {
        text: "Continue other work".to_string(),
        text_elements: Vec::new(),
    }])
}
async fn launch(
    server: &MockServer,
    test: &TestCodex,
    mode: &str,
    exit: i32,
) -> anyhow::Result<(i32, Option<String>)> {
    let command = format!(
        "i=0; while [ ! -f release ]; do i=$((i+1)); [ $i -lt 400 ] || exit 97; sleep .03; done; printf 'retained-command-output'; exit {exit}"
    );
    let args = json!({"cmd": command, "yield_time_ms": if mode == "await_exec" {10000} else if mode == "late_yield" {1500} else {250}, "login":false});
    let call = if mode == "direct" {
        ev_function_call("command", "exec_command", &args.to_string())
    } else {
        let after = if mode == "await_poll" {
            "text(await tools.write_stdin({session_id: r.session_id, chars: '', yield_time_ms: 10000}));"
        } else if mode == "await_exec" {
            "text(r);"
        } else {
            ""
        };
        responses::ev_custom_tool_call(
            "command",
            "exec",
            &format!(
                "// @exec: {{\"yield_time_ms\": 1000}}\nconst r = await tools.exec_command({args}); text('command-id=' + r.session_id); {after}"
            ),
        )
    };
    responses::mount_sse_once(server, sse(vec![call, ev_completed("launch")])).await;
    let yielded = responses::mount_sse_once(server, responses::sse_completed("idle")).await;
    test.submit_turn("Start work and finish while it runs")
        .await?;
    let request = yielded.single_request();
    let output = if mode == "direct" {
        request.function_call_output("command")
    } else {
        request.custom_tool_call_output("command")
    };
    let text = output.to_string();
    let id = regex_lite::Regex::new(r"(?:session ID |command-id=)([0-9]+)")?
        .captures(&text)
        .map(|c| c[1].parse::<i32>().expect("process ID"))
        .unwrap_or(-1);
    let cell = regex_lite::Regex::new(r"Script running with cell ID ([a-zA-Z0-9_.:-]+)")?
        .captures(&text)
        .map(|c| c[1].to_string());
    assert!(
        id >= 0 || cell.is_some(),
        "expected a running command or Cell: {output}"
    );
    Ok((id, cell))
}
async fn quiet(mock: &ResponseMock) {
    tokio::time::sleep(Duration::from_millis(/*millis*/ 150)).await;
    assert!(
        mock.requests().is_empty(),
        "no inference before ready or while busy/suppressed"
    );
}
async fn finish(test: &TestCodex) {
    wait_for_event(&test.codex, |e| matches!(e, EventMsg::TurnComplete(_))).await;
}
fn release(test: &TestCodex) {
    std::fs::write(test.cwd.path().join("release"), "ready").expect("release command");
}

#[test_case("direct", 0; "idle_success")]
#[test_case("direct", 7; "idle_nonzero")]
#[test_case("detached", 0; "command_outlives_completed_cell")]
#[test_case("await_exec", 0; "cell_owns_awaited_exec")]
#[test_case("await_poll", 7; "cell_owns_awaited_write_stdin")]
#[tokio::test]
async fn completion_preserves_output_and_nested_ownership(
    mode: &str,
    exit: i32,
) -> anyhow::Result<()> {
    core_test_support::skip_if_remote!(
        Ok(()),
        "wake rollout and release-file fixture are local only"
    );
    let server = responses::start_mock_server().await;
    let test = build(&server, mode != "direct", /*enabled*/ true).await?;
    let (id, cell) = launch(&server, &test, mode, exit).await?;
    let awaited = mode.starts_with("await_");
    assert_eq!(cell.is_some(), awaited);
    let read = if let Some(cell) = &cell {
        ev_function_call(
            "retrieve",
            "wait",
            &json!({"cell_id":cell,"yield_time_ms":1}).to_string(),
        )
    } else if mode == "detached" {
        responses::ev_custom_tool_call(
            "retrieve",
            "exec",
            &format!(
                "text(await tools.write_stdin({{session_id:{id},chars:'',yield_time_ms:1000}}));"
            ),
        )
    } else {
        ev_function_call(
            "retrieve",
            "write_stdin",
            &json!({"session_id":id,"chars":"","yield_time_ms":1000}).to_string(),
        )
    };
    let wake = responses::mount_sse_once(&server, sse(vec![read, ev_completed("wake")])).await;
    let result = responses::mount_sse_once(&server, responses::sse_completed("read")).await;
    quiet(&wake).await;
    release(&test);
    finish(&test).await;
    let wake_request = wake.single_request();
    let notice = wake_request
        .message_input_texts("user")
        .pop()
        .expect("completion notice");
    assert!(notice.contains(if awaited {
        "<code_mode_completion>"
    } else {
        "<exec_completion>"
    }));
    assert!(!notice.contains("retained-command-output"));
    let request = result.single_request();
    let output = if mode == "detached" {
        request.custom_tool_call_output("retrieve")
    } else {
        request.function_call_output("retrieve")
    };
    assert!(output.to_string().contains("retained-command-output"));
    if !awaited {
        assert!(
            output
                .to_string()
                .contains(&format!("Process exited with code {exit}"))
                || output.to_string().contains("exit_code")
        );
    }
    let duplicate =
        responses::mount_sse_once(&server, responses::sse_completed("unexpected-duplicate")).await;
    quiet(&duplicate).await;
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case(false; "busy_deferral")]
#[test_case(true; "manual_observation_retires")]
#[tokio::test]
async fn pending_command_does_not_steer_busy_turn(manual: bool) -> anyhow::Result<()> {
    core_test_support::skip_if_remote!(Ok(()), "local completion rollout");
    let server = responses::start_mock_server().await;
    let test = build(&server, /*nested*/ false, /*enabled*/ true).await?;
    let (id, _) = launch(&server, &test, "direct", /*exit*/ 0).await?;
    let response = if manual {
        sse(vec![
            ev_function_call(
                "manual",
                "write_stdin",
                &json!({"session_id":id,"chars":"","yield_time_ms":1000}).to_string(),
            ),
            ev_completed("busy"),
        ])
    } else {
        responses::sse_completed("busy")
    };
    let busy = responses::mount_response_sequence(
        &server,
        vec![responses::sse_response(response).set_delay(Duration::from_millis(/*millis*/ 700))],
    )
    .await;
    let read = if manual {
        Some(responses::mount_sse_once(&server, responses::sse_completed("observed")).await)
    } else {
        None
    };
    let wake = responses::mount_sse_once(&server, responses::sse_completed("wake")).await;
    test.codex.start_or_steer_turn(user()).await?;
    wait_for_event(&test.codex, |e| matches!(e, EventMsg::TurnStarted(_))).await;
    request_started(&busy).await;
    release(&test);
    wait_for_event(&test.codex, |e| matches!(e, EventMsg::ExecCommandEnd(_))).await;
    quiet(&wake).await;
    finish(&test).await;
    assert!(
        !busy
            .single_request()
            .body_contains_text("<exec_completion>")
    );
    if let Some(read) = read {
        assert!(
            read.single_request()
                .function_call_output("manual")
                .to_string()
                .contains("retained-command-output")
        );
        quiet(&wake).await;
    } else {
        finish(&test).await;
        assert!(
            wake.single_request()
                .body_contains_text("<exec_completion>")
        );
    }
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case("kill")]
#[test_case("shutdown")]
#[test_case("opt_out")]
#[test_case("interrupt")]
#[test_case("error")]
#[tokio::test]
async fn cancellation_and_inactive_clients_do_not_wake(reason: &str) -> anyhow::Result<()> {
    core_test_support::skip_if_remote!(Ok(()), "local completion rollout");
    let server = responses::start_mock_server().await;
    let test = build(&server, /*nested*/ false, reason != "opt_out").await?;
    let (id, _) = launch(&server, &test, "direct", /*exit*/ 0).await?;
    if matches!(reason, "interrupt" | "error") {
        let response = if reason == "error" {
            responses::sse_failed("failed", "invalid_prompt", "test failure")
        } else {
            responses::sse_completed("busy")
        };
        let busy = responses::mount_response_sequence(
            &server,
            vec![
                responses::sse_response(response)
                    .set_delay(Duration::from_millis(/*millis*/ 700)),
            ],
        )
        .await;
        test.codex.start_or_steer_turn(user()).await?;
        wait_for_event(&test.codex, |e| matches!(e, EventMsg::TurnStarted(_))).await;
        request_started(&busy).await;
        if reason == "interrupt" {
            test.codex.submit(Op::Interrupt).await?;
            wait_for_event(&test.codex, |e| matches!(e, EventMsg::TurnAborted(_))).await;
        } else {
            finish(&test).await;
        }
    }
    let wake = responses::mount_sse_once(&server, responses::sse_completed("unexpected")).await;
    if reason == "kill" {
        assert!(test.codex.terminate_background_terminal(id).await);
    }
    if reason == "shutdown" {
        test.codex.shutdown_and_wait().await?;
    }
    release(&test);
    if reason != "shutdown" {
        wait_for_event(&test.codex, |e| matches!(e, EventMsg::ExecCommandEnd(_))).await;
    }
    quiet(&wake).await;
    if reason != "shutdown" {
        test.codex.shutdown_and_wait().await?;
    }
    Ok(())
}

async fn request_started(mock: &ResponseMock) {
    tokio::time::timeout(Duration::from_secs(/*secs*/ 5), async {
        while mock.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(/*millis*/ 5)).await;
        }
    })
    .await
    .expect("busy request started");
}

#[tokio::test]
async fn command_yield_after_wrapper_exposure_survives_cell_wait_and_cleanup() -> anyhow::Result<()>
{
    core_test_support::skip_if_remote!(Ok(()), "local completion rollout");
    let server = responses::start_mock_server().await;
    let test = build(&server, /*nested*/ true, /*enabled*/ true).await?;
    let (_, cell) = launch(&server, &test, "late_yield", /*exit*/ 0).await?;
    let cell = cell.expect("wrapper yielded before command");
    let cell_wake = responses::mount_sse_once(
        &server,
        sse(vec![
            ev_function_call(
                "cell-read",
                "wait",
                &json!({"cell_id":cell,"yield_time_ms":1}).to_string(),
            ),
            ev_completed("cell-wake"),
        ]),
    )
    .await;
    let cell_result =
        responses::mount_sse_once(&server, responses::sse_completed("cell-read")).await;
    finish(&test).await;
    assert!(
        cell_wake
            .single_request()
            .body_contains_text("<code_mode_completion>")
    );
    let output = cell_result
        .single_request()
        .function_call_output("cell-read")
        .to_string();
    let id = regex_lite::Regex::new(r"command-id=([0-9]+)")?
        .captures(&output)
        .expect("detached command ID")[1]
        .to_string();
    let command_wake = responses::mount_sse_once(&server, sse(vec![responses::ev_custom_tool_call("command-read", "exec", &format!("text(await tools.write_stdin({{session_id:{id},chars:'',yield_time_ms:1000}}));")), ev_completed("command-wake")])).await;
    let result = responses::mount_sse_once(&server, responses::sse_completed("command-read")).await;
    quiet(&command_wake).await;
    release(&test);
    finish(&test).await;
    assert!(
        command_wake
            .single_request()
            .body_contains_text("<exec_completion>")
    );
    assert!(
        result
            .single_request()
            .custom_tool_call_output("command-read")
            .to_string()
            .contains("retained-command-output")
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
