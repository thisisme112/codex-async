use crate::grpc;
use crate::host::WireExecuteRequest;
use pretty_assertions::assert_eq;
use prost::Message;
use serde_json::json;

#[test]
fn old_wire_shapes_opt_out_without_changing_execute_payloads() {
    let legacy = json!({
        "tool_call_id": "call", "enabled_tools": [], "source": "text(1)",
        "yield_time_ms": null, "max_output_tokens": null
    });
    let request: WireExecuteRequest = serde_json::from_value(legacy.clone()).unwrap();
    assert!(!request.completion_ready);
    assert_eq!(serde_json::to_value(request).unwrap(), legacy);

    // An old host's SessionOpened contains only field 1 (session ID).
    let opened = grpc::SessionOpened::decode(&b"\x0a\x01s"[..]).unwrap();
    assert_eq!(
        opened,
        grpc::SessionOpened {
            session_id: "s".to_string(),
            output_truncation: None,
            completion_ready: false
        }
    );
    assert!(
        !grpc::OpenSessionRequest::decode(&b""[..])
            .unwrap()
            .completion_ready
    );
}

#[test]
fn upstream_truncation_and_callback_capacity_do_not_opt_in_to_completion() {
    let request = grpc::OpenSessionRequest::decode(&b"\x10\x02\x18\x80\x08"[..]).unwrap();
    assert_eq!(
        request,
        grpc::OpenSessionRequest {
            cell_execution_limits: None,
            output_truncation: Some(grpc::OutputTruncation::Server.into()),
            max_pending_delegate_calls: Some(1024),
            completion_ready: false,
        }
    );
    let opened = grpc::SessionOpened::decode(&b"\x0a\x01s\x10\x02"[..]).unwrap();
    assert_eq!(
        opened,
        grpc::SessionOpened {
            session_id: "s".to_string(),
            output_truncation: Some(grpc::OutputTruncation::Server.into()),
            completion_ready: false,
        }
    );
    let opted_in = grpc::OpenSessionRequest {
        completion_ready: true,
        ..request
    };
    assert_eq!(
        grpc::OpenSessionRequest::decode(opted_in.encode_to_vec().as_slice()).unwrap(),
        opted_in
    );
}
