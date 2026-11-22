use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn session_opt_in_is_required_before_sending_the_new_event() {
    for requested in [false, true] {
        let host = GrpcCodeModeHost::new();
        let mut events = host
            .open_session(Request::new(proto::OpenSessionRequest {
                cell_execution_limits: None,
                output_truncation: None,
                max_pending_delegate_calls: None,
                heartbeat_interval_s: 0,
                completion_ready: requested,
            }))
            .await
            .unwrap()
            .into_inner();
        let proto::session_event::Event::Opened(opened) =
            events.next().await.unwrap().unwrap().event.unwrap()
        else {
            panic!("session opening")
        };
        assert_eq!(opened.completion_ready, requested);
        assert!(opened.supports_catalog_revision);
        for (execution_id, install_catalog) in [("install", true), ("reuse", false)] {
            let mut request =
                super::tests::execute_request(&opened.session_id, execution_id, "text('done');");
            request.completion_ready = true;
            request.catalog_revision = Some(42);
            if install_catalog {
                request.enabled_tools.push(super::tests::tool("echo"));
            }
            request.yield_time_ms = Some(10_000);
            let mut execution = host
                .execute(Request::new(request))
                .await
                .unwrap()
                .into_inner();
            while let Some(event) = execution.next().await {
                event.unwrap();
            }
            let first = events.next().await.unwrap().unwrap().event.unwrap();
            if requested {
                assert!(matches!(
                    first,
                    proto::session_event::Event::CellCompletionReady(ready)
                        if ready.execution_id == execution_id
                ));
                assert!(matches!(
                    events.next().await.unwrap().unwrap().event,
                    Some(proto::session_event::Event::CellClosed(closed))
                        if closed.execution_id == execution_id
                ));
            } else {
                assert!(matches!(
                    first,
                    proto::session_event::Event::CellClosed(closed)
                        if closed.execution_id == execution_id
                ));
            }
        }
        host.close_session(Request::new(proto::CloseSessionRequest {
            session_id: opened.session_id,
        }))
        .await
        .unwrap();
    }
}
