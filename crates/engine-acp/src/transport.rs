//! The ACP agent's supervised transport: the shared [`SupervisedTransport`]
//! with ACP's event classification and nouns.

use interface_engine::{
    jsonrpc::{FrameError, RouteEvent},
    supervised::{Classify, Connection, SupervisedTransport, TransportLimits},
};

use crate::protocol::{AcpInbound, classify_route};

pub use interface_engine::supervised::{
    DEFAULT_INBOUND_CAPACITY, DEFAULT_MAX_FRAME_BYTES, DEFAULT_MAX_PENDING,
    DEFAULT_OUTBOUND_CAPACITY, DEFAULT_REQUEST_TIMEOUT,
};

pub type AcpTransportLimits = TransportLimits;

pub type AcpTransport = SupervisedTransport<AcpTransportEvent>;

pub type AcpConnection = Connection<AcpTransportEvent>;

#[derive(Clone, Debug, PartialEq)]
pub enum AcpTransportEvent {
    Inbound(AcpInbound),
    Closed(String),
}

impl Classify for AcpTransportEvent {
    const LABEL: &'static str = "ACP";
    const CHILD: &'static str = "ACP agent";

    fn classify(decoded: Result<RouteEvent, FrameError>) -> Self {
        Self::Inbound(match decoded {
            Ok(route) => classify_route(route),
            Err(error) => AcpInbound::FramingError(error),
        })
    }

    fn closed(message: String) -> Self {
        Self::Closed(message)
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use interface_engine::{DeliveryOutcome, ProcessSpec, jsonrpc::RpcNotification};
    use serde_json::{Value, json};
    use tokio::time::{sleep, timeout};

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    const BURST: &str = r#"
import json, sys
sys.stdout.write("".join(json.dumps({"method": "burst", "params": {"n": n}}) + "\n" for n in range(100)))
sys.stdout.write("x" * 2048 + "\n")
sys.stdout.write(json.dumps({"method": "after", "params": {}}) + "\n")
sys.stdout.flush()
for line in sys.stdin:
    message = json.loads(line)
    print(json.dumps({"id": message["id"], "result": {"echo": message["method"]}}), flush=True)
"#;

    fn python(script: &str) -> ProcessSpec {
        ProcessSpec {
            program: "/usr/bin/env".into(),
            args: vec!["python3".into(), "-c".into(), script.into()],
            cwd: None,
        }
    }

    fn notification(method: &str, params: Value) -> AcpTransportEvent {
        AcpTransportEvent::Inbound(AcpInbound::Notification(RpcNotification {
            method: method.into(),
            params: Some(params),
        }))
    }

    #[tokio::test]
    async fn a_burst_while_the_session_is_busy_waits_instead_of_failing_the_transport() {
        let limits = AcpTransportLimits {
            inbound_capacity: 8,
            max_frame_bytes: 1_024,
            request_timeout: WAIT,
            ..AcpTransportLimits::default()
        };
        let AcpConnection {
            transport,
            mut inbound,
        } = AcpTransport::spawn(&python(BURST), &limits).unwrap();
        // Nothing drains the queue while more than a queue's worth arrives at once.
        sleep(Duration::from_millis(300)).await;
        let request = {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move { transport.request("ping", &json!({})).await })
        };
        for n in 0..100 {
            let event = timeout(WAIT, inbound.recv()).await.unwrap().unwrap();
            assert_eq!(event, notification("burst", json!({"n": n})));
        }
        // An oversize frame is still refused as a framing error, in order.
        assert_eq!(
            timeout(WAIT, inbound.recv()).await.unwrap().unwrap(),
            AcpTransportEvent::Inbound(AcpInbound::FramingError(FrameError::TooLarge {
                limit: 1_024
            }))
        );
        assert_eq!(
            timeout(WAIT, inbound.recv()).await.unwrap().unwrap(),
            notification("after", json!({}))
        );
        match timeout(WAIT, request).await.unwrap().unwrap() {
            DeliveryOutcome::Confirmed(result) => assert_eq!(result, json!({"echo": "ping"})),
            other => panic!("the request should settle after the burst: {other:?}"),
        }
        transport.shutdown();
    }

    #[test]
    fn transport_prose_keeps_the_acp_nouns() {
        assert_eq!(AcpTransportEvent::LABEL, "ACP");
        assert_eq!(AcpTransportEvent::CHILD, "ACP agent");
        assert_eq!(
            AcpTransportEvent::closed("gone".into()),
            AcpTransportEvent::Closed("gone".into())
        );
    }
}
