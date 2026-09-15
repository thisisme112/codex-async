use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn session_opt_in_is_required_before_sending_the_new_event() {
    for requested in [false, true] {
        let host = GrpcCodeModeHost::new();
        let mut events = host
            .open_session(Request::new(proto::OpenSessionRequest {
                cell_execution_limits: None,
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
        let mut request =
            super::tests::execute_request(&opened.session_id, "execution", "text('done');");
        request.completion_ready = true;
        request.yield_time_ms = Some(10_000);
        let mut execution = host
            .execute(Request::new(request))
            .await
            .unwrap()
            .into_inner();
        while execution.next().await.is_some() {}
        let first = events.next().await.unwrap().unwrap().event.unwrap();
        if requested {
            assert!(matches!(
                first,
                proto::session_event::Event::CellCompletionReady(_)
            ));
            assert!(matches!(
                events.next().await.unwrap().unwrap().event,
                Some(proto::session_event::Event::CellClosed(_))
            ));
        } else {
            assert!(matches!(first, proto::session_event::Event::CellClosed(_)));
        }
        host.close_session(Request::new(proto::CloseSessionRequest {
            session_id: opened.session_id,
        }))
        .await
        .unwrap();
    }
}
