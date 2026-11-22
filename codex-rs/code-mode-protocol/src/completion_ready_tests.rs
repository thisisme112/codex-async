use crate::grpc;
use pretty_assertions::assert_eq;
use prost::Message;

#[test]
fn old_wire_shapes_opt_out_of_completion() {
    // An old host's SessionOpened contains only field 1 (session ID).
    let opened = grpc::SessionOpened::decode(&b"\x0a\x01s"[..]).unwrap();
    assert_eq!(
        opened,
        grpc::SessionOpened {
            session_id: "s".to_string(),
            output_truncation: None,
            supports_catalog_revision: false,
            completion_ready: false,
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
            heartbeat_interval_s: 0,
            completion_ready: false,
        }
    );
    let opened = grpc::SessionOpened::decode(&b"\x0a\x01s\x10\x02"[..]).unwrap();
    assert_eq!(
        opened,
        grpc::SessionOpened {
            session_id: "s".to_string(),
            output_truncation: Some(grpc::OutputTruncation::Server.into()),
            supports_catalog_revision: false,
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

#[test]
fn legacy_completion_opt_in_does_not_enable_the_new_event_format() {
    // Earlier peers used tags 100/101. They cannot be allowed to opt in
    // to the new ExecuteRequest encoding.
    for old_tag in [0xa0, 0xa8] {
        let legacy_opt_in = [old_tag, 0x06, 0x01];
        assert!(
            !grpc::OpenSessionRequest::decode(legacy_opt_in.as_slice())
                .unwrap()
                .completion_ready
        );
        assert!(
            !grpc::SessionOpened::decode(legacy_opt_in.as_slice())
                .unwrap()
                .completion_ready
        );
    }
}

#[test]
fn execute_catalog_revision_and_completion_use_distinct_wire_fields() {
    // Field 8 is upstream's optional uint64 catalog_revision; a stock peer
    // that supplies it must not also opt in to our completion event.
    let upstream_wire = [0x40, 0x2a];
    let mut request = grpc::ExecuteRequest::decode(upstream_wire.as_slice()).unwrap();
    assert_eq!(request.catalog_revision, Some(42));
    assert!(!request.completion_ready);

    // Field 101 is our explicit opt-in bit. Preserve both fields together.
    request.completion_ready = true;
    let encoded = request.encode_to_vec();
    assert_eq!(encoded, [0x40, 0x2a, 0xa8, 0x06, 0x01]);
    assert_eq!(
        grpc::ExecuteRequest::decode(encoded.as_slice()).unwrap(),
        request
    );
}
