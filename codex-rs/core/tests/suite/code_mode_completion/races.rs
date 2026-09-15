use super::*;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::TurnErrorInput;
use codex_extension_api::TurnLifecycleContributor;
use tokio::sync::Notify;

#[derive(Default)]
struct ErrorGate {
    observed: Notify,
    release: Notify,
}
impl TurnLifecycleContributor for ErrorGate {
    fn on_turn_error<'a>(&'a self, _input: TurnErrorInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            self.observed.notify_one();
            self.release.notified().await;
        })
    }
}

#[tokio::test]
async fn explicit_continuation_after_error_observation_survives_later_idle_handoff()
-> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let session = Arc::new(ControlledSession {
        supported: true,
        ..Default::default()
    });
    let gate = Arc::new(ErrorGate::default());
    let mut extensions = ExtensionRegistryBuilder::new();
    extensions.turn_lifecycle_contributor(gate.clone());
    let test = fixture::builder(session.clone(), /*client*/ true)
        .with_extensions(Arc::new(extensions.build()))
        .build_with_auto_env(&server)
        .await?;
    launch(&server, &test, "held").await;
    responses::mount_sse_once(
        &server,
        responses::sse_failed("failure", "invalid_prompt", "test failure"),
    )
    .await;
    test.codex.start_or_steer_turn(user("Other work")).await?;
    tokio::time::timeout(Duration::from_secs(5), gate.observed.notified()).await?;
    let wake = responses::mount_sse_once(&server, responses::sse_completed("wake")).await;
    assert!(matches!(
        test.codex
            .start_or_steer_turn(user("Continue after the error"))
            .await?,
        codex_core::TurnInputSubmission::Steered { .. }
    ));
    session.complete();
    no_requests(&wake).await;
    gate.release.notify_one();
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(
        wake.single_request()
            .body_contains_text("<code_mode_completion>")
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
