use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::StartedCell;
use codex_code_mode_protocol::host::DelegateRequest;
use codex_code_mode_protocol::host::EncodedFrame;
use codex_code_mode_protocol::host::HostResponse;
use codex_code_mode_protocol::host::HostToClient;
use codex_code_mode_protocol::host::MAX_PENDING_DELEGATE_CALLS;
use codex_code_mode_protocol::host::RequestId;
use codex_code_mode_protocol::host::SessionId;
use codex_code_mode_protocol::host::WireResult;
use pretty_assertions::assert_eq;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::oneshot::error::TryRecvError;
use tokio_util::sync::CancellationToken;

use super::HostPeer;

fn session_id(value: &str) -> SessionId {
    SessionId::new(value).expect("session ID")
}

#[tokio::test]
async fn completion_ready_before_start_cell_preserves_admission_and_closure_order() {
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel(/*buffer*/ 4);
    let mut peer = HostPeer::new(outgoing_tx, MAX_PENDING_DELEGATE_CALLS);
    peer.completion_ready = true;
    let peer = Arc::new(peer);
    let session_id = session_id("session-1");
    let cell_id = CellId::new("cell-1".to_string());
    let request_id = RequestId::new(/*value*/ 1);
    let (response_tx, response_rx) = oneshot::channel();
    let started = StartedCell::new(cell_id.clone(), response_rx);
    let permits = Arc::new(Semaphore::new(/*permits*/ 1));
    let permit = Arc::clone(&permits)
        .try_acquire_owned()
        .expect("active cell permit");

    // Force both callbacks into the Pending route before admission. The initial
    // response stays gated so this cannot accidentally test only the Active route.
    peer.completion_ready(session_id.clone(), cell_id.clone());
    peer.close_cell(session_id.clone(), cell_id.clone());
    assert!(outgoing_rx.try_recv().is_err());
    let admission = HostResponse::ExecutionStarted {
        cell_id: (&cell_id).into(),
    };
    peer.respond(request_id, Ok(admission));
    let mut initial_response_sent = peer.start_cell(
        session_id.clone(),
        request_id,
        started,
        permit,
        Instant::now(),
    );

    let expected_before_response = [
        HostToClient::Response {
            id: request_id,
            result: WireResult::Ok {
                value: HostResponse::ExecutionStarted {
                    cell_id: (&cell_id).into(),
                },
            },
        },
        HostToClient::CellCompletionReady {
            session_id: session_id.clone(),
            cell_id: (&cell_id).into(),
        },
    ];
    for expected in expected_before_response {
        let frame = tokio::time::timeout(Duration::from_secs(/*secs*/ 1), outgoing_rx.recv())
            .await
            .expect("admission/readiness timeout")
            .expect("admission/readiness frame");
        assert_eq!(
            EncodedFrame::decode_framed::<HostToClient>(&frame.into_framed_bytes())
                .expect("decode host frame"),
            expected,
        );
    }
    assert_eq!(initial_response_sent.try_recv(), Err(TryRecvError::Empty));
    assert!(outgoing_rx.try_recv().is_err());

    let response = RuntimeResponse::Result {
        cell_id: cell_id.clone(),
        content_items: Vec::new(),
        error_text: None,
        code_mode_host_duration: None,
    };
    response_tx
        .send(response.clone())
        .expect("initial response receiver");
    initial_response_sent
        .await
        .expect("initial response enqueued");
    let released = tokio::time::timeout(Duration::from_secs(/*secs*/ 1), permits.acquire())
        .await
        .expect("cell forwarding finished")
        .expect("cell permit released");
    drop(released);

    let initial_frame = outgoing_rx.try_recv().expect("initial response frame");
    let HostToClient::InitialResponse { id, result } =
        EncodedFrame::decode_framed(&initial_frame.into_framed_bytes())
            .expect("decode initial response")
    else {
        panic!("expected initial response before cleanup");
    };
    let actual: RuntimeResponse = result.into_result().expect("initial result").into();
    assert_eq!(
        (id, actual.with_code_mode_host_duration(Duration::ZERO)),
        (
            request_id,
            response.with_code_mode_host_duration(Duration::ZERO)
        ),
    );
    let closed_frame = outgoing_rx.try_recv().expect("cleanup frame");
    assert_eq!(
        EncodedFrame::decode_framed::<HostToClient>(&closed_frame.into_framed_bytes())
            .expect("decode cleanup"),
        HostToClient::CellClosed {
            session_id,
            cell_id: cell_id.into(),
        },
    );
    // The permit is released only after forwarding finishes, so this assertion
    // detects duplicate readiness without relying on scheduler delays.
    assert!(outgoing_rx.try_recv().is_err());
    assert!(!peer.is_disconnected());
}

#[tokio::test]
async fn start_cell_reports_when_initial_response_is_enqueued() {
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel(/*max_capacity*/ 4);
    let peer = Arc::new(HostPeer::new(outgoing_tx, MAX_PENDING_DELEGATE_CALLS));
    let cell_id = CellId::new("cell-1".to_string());
    let (response_tx, response_rx) = oneshot::channel();
    let started = StartedCell::new(cell_id.clone(), response_rx);
    let active_cell_permits = Arc::new(Semaphore::new(/*permits*/ 1));
    let active_cell_permit = Arc::clone(&active_cell_permits)
        .try_acquire_owned()
        .expect("active cell permit");

    let mut initial_response_sent = peer.start_cell(
        session_id("session-1"),
        RequestId::new(/*value*/ 1),
        started,
        active_cell_permit,
        Instant::now(),
    );
    assert_eq!(initial_response_sent.try_recv(), Err(TryRecvError::Empty));

    response_tx
        .send(RuntimeResponse::Result {
            code_mode_host_duration: None,
            cell_id: cell_id.clone(),
            content_items: Vec::new(),
            result_buffered_for: Duration::ZERO,
            error_text: None,
        })
        .expect("initial response receiver");
    initial_response_sent
        .await
        .expect("initial response completion");
    outgoing_rx.recv().await.expect("initial response frame");
    assert_eq!(active_cell_permits.available_permits(), 0);

    peer.close_cell(session_id("session-1"), cell_id);
    let permit = tokio::time::timeout(
        Duration::from_secs(1),
        Arc::clone(&active_cell_permits).acquire_owned(),
    )
    .await
    .expect("cell permit should be released")
    .expect("cell permit semaphore should remain open");
    drop(permit);
}

#[tokio::test]
async fn pending_delegate_limit_rejects_call_without_disconnecting() {
    let (outgoing_tx, _outgoing_rx) = mpsc::channel(/*max_capacity*/ 1);
    let peer = Arc::new(HostPeer::new(outgoing_tx, MAX_PENDING_DELEGATE_CALLS));
    let permits = Arc::clone(&peer.delegate_permits)
        .acquire_many_owned(MAX_PENDING_DELEGATE_CALLS as u32)
        .await
        .expect("delegate permits");

    let result = peer
        .call(
            session_id("session-1"),
            DelegateRequest::Notify {
                call_id: "call-1".to_string(),
                cell_id: CellId::new("cell-1".to_string()).into(),
                text: "hello".to_string(),
            },
            CancellationToken::new(),
        )
        .await;

    assert_eq!(
        result,
        Err("code-mode host has too many pending delegate calls".to_string())
    );
    assert!(!peer.is_disconnected());
    drop(permits);
}
