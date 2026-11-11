#[path = "code_mode_completion/fixture.rs"]
mod fixture;
#[path = "code_mode_completion/mcp.rs"]
mod mcp;
#[path = "code_mode_completion/races.rs"]
mod races;

use std::sync::Arc;
use std::time::Duration;

use codex_core::TurnInputRequest;
use codex_features::Feature;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_custom_tool_call;
use core_test_support::responses::sse;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use test_case::test_case;
use wiremock::MockServer;

use fixture::ControlledSession;

async fn launch(server: &MockServer, test: &TestCodex, code: &str) -> ResponseMock {
    responses::mount_sse_once(
        server,
        sse(vec![
            ev_custom_tool_call("exec-1", "exec", code),
            ev_completed("exec"),
        ]),
    )
    .await;
    let output = responses::mount_sse_once(server, responses::sse_completed("yielded")).await;
    test.submit_turn("Run this cell and finish the turn while it runs")
        .await
        .expect("launch yielded cell");
    output
}

fn user(text: &str) -> TurnInputRequest {
    TurnInputRequest::user_input(vec![UserInput::Text {
        text: text.to_string(),
        text_elements: Vec::new(),
    }])
}

fn running_cell_id(request: &responses::ResponsesRequest) -> String {
    let output = request.custom_tool_call_output("exec-1");
    let output = &output["output"];
    let text = output
        .as_str()
        .or_else(|| output[0]["text"].as_str())
        .expect("yielded tool text");
    text.strip_prefix("Script running with cell ID ")
        .expect("yielded cell")
        .split('\n')
        .next()
        .expect("cell ID line")
        .to_string()
}

async fn no_requests(mock: &ResponseMock) {
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        mock.requests().is_empty(),
        "completion must not poll or inject while busy/disabled"
    );
}

async fn wake_and_wait(server: &MockServer, cell_id: &str) -> (ResponseMock, ResponseMock) {
    let wake = responses::mount_sse_once(
        server,
        sse(vec![
            responses::ev_function_call(
                "wait-1",
                "wait",
                &serde_json::json!({"cell_id":cell_id,"yield_time_ms":1}).to_string(),
            ),
            ev_completed("wake"),
        ]),
    )
    .await;
    let result = responses::mount_sse_once(server, responses::sse_completed("retrieved")).await;
    (wake, result)
}

#[tokio::test]
async fn idle_completion_starts_once_without_polling_and_wait_owns_the_result() -> anyhow::Result<()>
{
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported: true,
        ..Default::default()
    });
    let test = fixture::build(&server, session.clone(), /*client*/ true).await?;
    let output = launch(&server, &test, "held").await;
    assert!(
        output
            .single_request()
            .body_contains_text("can resume automatically once idle")
    );
    let (wake, result) = wake_and_wait(&server, "generation:1").await;
    no_requests(&wake).await;
    session.complete();
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let request = wake.single_request();
    assert!(
        request.body_contains_text("<code_mode_completion>Code Mode cells completed: generation:1")
    );
    assert!(!request.body_contains_text("retained result"));
    assert!(
        result
            .single_request()
            .body_contains_text("retained result")
    );
    assert_eq!(wake.requests().len(), 1);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case(true, false; "client_opted_out")]
#[test_case(false, true; "unsupported_provider")]
#[tokio::test]
async fn opt_out_preserves_exec_and_wait(supported: bool, client: bool) -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported,
        ..Default::default()
    });
    let test = fixture::build(&server, session.clone(), client).await?;
    let output = launch(&server, &test, "held").await;
    assert!(
        !output
            .single_request()
            .body_contains_text("can resume automatically")
    );
    let (next, result) = wake_and_wait(&server, "generation:1").await;
    session.complete();
    no_requests(&next).await;
    test.submit_turn("Retrieve the result manually").await?;
    assert!(
        !next
            .single_request()
            .body_contains_text("<code_mode_completion>")
    );
    assert!(
        result
            .single_request()
            .body_contains_text("retained result")
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn ready_before_initial_yield_handoff_waits_for_exposure_and_idle() -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported: true,
        early: true,
        ..Default::default()
    });
    let test = fixture::build(&server, session, /*client*/ true).await?;
    // Install all expected responses before early readiness can fire.
    responses::mount_sse_once(
        &server,
        sse(vec![
            ev_custom_tool_call("exec-1", "exec", "early"),
            ev_completed("exec"),
        ]),
    )
    .await;
    let output = responses::mount_sse_once(&server, responses::sse_completed("yielded")).await;
    let (wake, result) = wake_and_wait(&server, "generation:1").await;
    test.submit_turn("Run and finish").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(
        !output
            .single_request()
            .body_contains_text("<code_mode_completion>")
    );
    assert!(
        wake.single_request()
            .body_contains_text("Script running with cell ID generation:1")
    );
    assert!(
        result
            .single_request()
            .body_contains_text("retained result")
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case("text('timer result');", "timer result"; "success")]
#[test_case("throw new Error('timer error');", "timer error"; "script_error")]
#[tokio::test]
async fn real_timer_cell_wakes_an_ordinary_idle_thread(
    finish: &str,
    expected: &str,
) -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let test = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_config(|config| {
            config
                .features
                .enable(Feature::CodeMode)
                .expect("enable Code Mode");
        })
        .build_with_auto_env(&server)
        .await?;
    test.codex.enable_code_mode_completion_wake();
    let output = launch(
        &server,
        &test,
        &format!(
            "yield_control(); await new Promise(resolve => setTimeout(resolve, 2000)); {finish}"
        ),
    )
    .await;
    let request = output.single_request();
    let cell_id = running_cell_id(&request);
    let (wake, result) = wake_and_wait(&server, &cell_id).await;
    no_requests(&wake).await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(
        wake.single_request()
            .body_contains_text("<code_mode_completion>")
    );
    assert!(result.single_request().body_contains_text(expected));
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case(false; "busy_deferral")]
#[test_case(true; "manual_wait_consumes_pending")]
#[tokio::test]
async fn completion_never_injects_into_a_busy_turn(manual_wait: bool) -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported: true,
        ..Default::default()
    });
    let test = fixture::build(&server, session.clone(), /*client*/ true).await?;
    launch(&server, &test, "held").await;
    let busy_response = if manual_wait {
        sse(vec![
            responses::ev_function_call(
                "manual-wait",
                "wait",
                r#"{"cell_id":"generation:1","yield_time_ms":1}"#,
            ),
            ev_completed("busy"),
        ])
    } else {
        responses::sse_completed("busy")
    };
    let busy = responses::mount_response_sequence(
        &server,
        vec![responses::sse_response(busy_response).set_delay(Duration::from_millis(500))],
    )
    .await;
    let manual_result = if manual_wait {
        Some(responses::mount_sse_once(&server, responses::sse_completed("manual-result")).await)
    } else {
        None
    };
    let wake = responses::mount_sse_once(&server, responses::sse_completed("completion")).await;
    test.codex.start_or_steer_turn(user("Other work")).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnStarted(_))
    })
    .await;
    session.complete();
    no_requests(&wake).await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(
        !busy
            .single_request()
            .body_contains_text("<code_mode_completion>")
    );
    if let Some(manual_result) = manual_result {
        no_requests(&wake).await;
        assert!(
            manual_result
                .single_request()
                .body_contains_text("retained result")
        );
    } else {
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnComplete(_))
        })
        .await;
        assert!(
            wake.single_request()
                .body_contains_text("<code_mode_completion>")
        );
    }
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[test_case(false; "retained_cells")]
#[test_case(true; "default_terminated_cells")]
#[tokio::test]
async fn interrupted_thread_defers_until_explicit_user_continuation(
    terminate_cells: bool,
) -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported: true,
        ..Default::default()
    });
    let test = fixture::builder(session.clone(), /*client*/ true)
        .with_config(move |config| {
            // Upstream now terminates cells on interrupt by default. Exercise
            // retained completion separately without weakening that default.
            if !terminate_cells {
                config
                    .features
                    .disable(Feature::CodeModeInterrupt)
                    .expect("retain interrupted cells for deferred completion");
            }
        })
        .build_with_auto_env(&server)
        .await?;
    launch(&server, &test, "held").await;
    let busy = responses::mount_response_sequence(
        &server,
        vec![
            responses::sse_response(responses::sse_completed("interrupted"))
                .set_delay(Duration::from_secs(2)),
        ],
    )
    .await;
    test.codex.start_or_steer_turn(user("Other work")).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnStarted(_))
    })
    .await;
    // Ensure this response has been dispatched before interrupting it.
    tokio::time::timeout(Duration::from_secs(5), async {
        while busy.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    test.codex.submit(Op::Interrupt).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;
    assert_eq!(
        session.delegate.lock().expect("delegate lock").is_none(),
        terminate_cells,
        "default interruption must terminate the cell and retire its callback"
    );
    let continuation =
        responses::mount_sse_once(&server, responses::sse_completed("user-continued")).await;
    session.complete();
    no_requests(&continuation).await;
    let wake = responses::mount_sse_once(&server, responses::sse_completed("wake")).await;
    test.submit_turn("Continue").await?;
    assert!(
        !continuation
            .single_request()
            .body_contains_text("<code_mode_completion>")
    );
    if terminate_cells {
        no_requests(&wake).await;
    } else {
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnComplete(_))
        })
        .await;
        assert!(
            wake.single_request()
                .body_contains_text("<code_mode_completion>")
        );
    }
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_retires_unfinished_cells_without_starting_a_turn() -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported: true,
        ..Default::default()
    });
    let test = fixture::build(&server, session.clone(), /*client*/ true).await?;
    launch(&server, &test, "held").await;
    let wake = responses::mount_sse_once(&server, responses::sse_completed("unexpected")).await;
    test.codex.shutdown_and_wait().await?;
    session.complete();
    no_requests(&wake).await;
    Ok(())
}

#[tokio::test]
async fn terminal_error_suppresses_completion_until_user_continues() -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported: true,
        ..Default::default()
    });
    let test = fixture::build(&server, session.clone(), /*client*/ true).await?;
    launch(&server, &test, "held").await;
    responses::mount_sse_once(
        &server,
        responses::sse_failed("failure", "invalid_prompt", "test terminal failure"),
    )
    .await;
    test.codex.start_or_steer_turn(user("Other work")).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let continuation =
        responses::mount_sse_once(&server, responses::sse_completed("user-continued")).await;
    session.complete();
    no_requests(&continuation).await;
    let wake = responses::mount_sse_once(&server, responses::sse_completed("wake")).await;
    test.submit_turn("Continue").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(
        !continuation
            .single_request()
            .body_contains_text("<code_mode_completion>")
    );
    assert!(
        wake.single_request()
            .body_contains_text("<code_mode_completion>")
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn plan_mode_retains_completion_until_user_leaves_plan() -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported: true,
        ..Default::default()
    });
    let test = fixture::build(&server, session.clone(), /*client*/ true).await?;
    launch(&server, &test, "held").await;
    let mut mode = test.codex.config_snapshot().await.collaboration_mode;
    let original = mode.clone();
    mode.mode = codex_protocol::config_types::ModeKind::Plan;
    core_test_support::submit_thread_settings(
        &test.codex,
        codex_protocol::protocol::ThreadSettingsOverrides {
            collaboration_mode: Some(mode),
            ..Default::default()
        },
    )
    .await?;
    let continuation =
        responses::mount_sse_once(&server, responses::sse_completed("user-continued")).await;
    session.complete();
    no_requests(&continuation).await;
    let wake = responses::mount_sse_once(&server, responses::sse_completed("wake")).await;
    test.codex
        .start_or_steer_turn(user("Continue outside Plan").with_thread_settings(
            codex_protocol::protocol::ThreadSettingsOverrides {
                collaboration_mode: Some(original),
                ..Default::default()
            },
        ))
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(
        !continuation
            .single_request()
            .body_contains_text("<code_mode_completion>")
    );
    assert!(
        wake.single_request()
            .body_contains_text("<code_mode_completion>")
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
