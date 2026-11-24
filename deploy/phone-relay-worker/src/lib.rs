//! The phone relay as a Cloudflare Worker. A Durable Object holds each allowed
//! Mac's outbound WebSocket (hibernating between messages) and carries phone
//! packets to it. Behaviour matches `crates/interface-remote/src/relay.rs`;
//! see `README.md` for the differences a Worker forces.
pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod relay {
    use crate::protocol::{
        self,
        relay_admission::{self, Waiting},
    };
    use futures::{
        channel::oneshot,
        future::{Either, select},
    };
    use interface_contracts::remote_connection::MAX_PACKET_BYTES;
    use serde::{Deserialize, Serialize};
    use std::{cell::RefCell, collections::BTreeSet, collections::HashMap, time::Duration};
    use worker::*;

    /// A phone's request waits this long for the Mac, as in `relay::forward`.
    const REPLY_WAIT: Duration = Duration::from_secs(12);
    /// Keepalives every 25 s keep each Mac inside its 50 s read deadline.
    const KEEPALIVE: Duration = Duration::from_secs(25);
    /// A socket that has not proved its key by then is closed.
    const PROOF_WAIT_MS: u64 = 10_000;
    const PENDING_PER_MAC: usize = 8;
    /// Sockets one address may have here that are not a proven Mac, counting
    /// displaced ones still closing. The runtime keeps a closed socket until
    /// its client answers the close, so a client that never answers would
    /// otherwise pile them up. A Mac answers at once and holds one or two.
    const HELD_PER_ADDRESS: usize = 32;

    #[event(fetch)]
    async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
        let response = match (req.method(), req.path().as_str()) {
            (Method::Get, "/health") => Response::ok("ok"),
            (Method::Get, "/v5/mac") | (Method::Post, "/v5/packet") => {
                // One object holds every Mac: a Mac names itself only after
                // its socket is open, so the socket cannot be routed by Mac.
                env.durable_object("RELAY")?
                    .get_by_name("relay")?
                    .fetch_with_request(req)
                    .await
            }
            _ => status(404),
        };
        // No cookies, CORS, request bodies or contents are logged or cached.
        let response = response.or_else(|_| status(500))?;
        if response.status_code() == 101 {
            return Ok(response);
        }
        // A Durable Object's response headers are immutable; copy them.
        let headers = Headers::new();
        for (name, value) in response.headers().entries() {
            headers.set(&name, &value)?;
        }
        headers.set("Cache-Control", "no-store")?;
        headers.set("X-Content-Type-Options", "nosniff")?;
        Ok(response.with_headers(headers))
    }

    fn status(code: u16) -> Result<Response> {
        Ok(Response::empty()?.with_status(code))
    }

    /// Per-socket state that survives hibernation.
    #[derive(Serialize, Deserialize)]
    struct Socket {
        id: String,
        opened: u64,
        mac_id: Option<String>,
        /// The challenge, until the Mac proves it.
        secret: Option<String>,
        ready: bool,
        /// The client address Cloudflare saw (`CF-Connecting-IP`).
        #[serde(default)]
        address: Option<String>,
        /// Displaced by a newer socket and being closed: it waits for nothing.
        #[serde(default)]
        closing: bool,
    }
    struct Pending {
        socket: String,
        mac_id: String,
        reply: oneshot::Sender<String>,
    }

    #[durable_object]
    pub struct Relay {
        state: State,
        env: Env,
        /// Phone requests in flight. Only exists while a request keeps the
        /// object awake, so it never needs to survive hibernation.
        pending: RefCell<HashMap<String, Pending>>,
    }

    impl DurableObject for Relay {
        fn new(state: State, env: Env) -> Self {
            Self {
                state,
                env,
                pending: RefCell::new(HashMap::new()),
            }
        }

        async fn fetch(&self, req: Request) -> Result<Response> {
            let Some(allowed) = self.allowed() else {
                console_error!("ALLOWED_MACS is missing or invalid");
                return status(503);
            };
            match req.path().as_str() {
                "/v5/mac" => self.open(&req).await,
                "/v5/packet" => self.forward(req, &allowed).await,
                _ => status(404),
            }
        }

        async fn websocket_message(
            &self,
            ws: WebSocket,
            message: WebSocketIncomingMessage,
        ) -> Result<()> {
            let Some(mut socket) = ws.deserialize_attachment::<Socket>()? else {
                return refuse(&ws);
            };
            if socket.closing {
                return refuse(&ws);
            }
            let WebSocketIncomingMessage::String(text) = message else {
                self.drop_socket(&socket.id);
                return refuse(&ws);
            };
            match (socket.ready, socket.secret.take()) {
                (true, _) => {
                    let mac_id = socket.mac_id.as_deref().unwrap_or_default();
                    let Some(packet) = protocol::mac_packet(&text, mac_id) else {
                        self.drop_socket(&socket.id);
                        return refuse(&ws);
                    };
                    let mut pending = self.pending.borrow_mut();
                    if pending
                        .get(&packet.request_id)
                        .is_some_and(|waiting| waiting.socket == socket.id)
                    {
                        if let Some(waiting) = pending.remove(&packet.request_id) {
                            let _ = waiting.reply.send(text);
                        }
                    }
                    Ok(())
                }
                (false, None) => {
                    let Some(allowed) = self.allowed() else {
                        return refuse(&ws);
                    };
                    let Some(challenge) = protocol::challenge(&text, &allowed) else {
                        return refuse(&ws);
                    };
                    socket.mac_id = Some(challenge.mac_id);
                    socket.secret = Some(challenge.secret);
                    ws.serialize_attachment(&socket)?;
                    ws.send(&challenge.packet)
                }
                (false, Some(secret)) => {
                    if !protocol::proves(&text, &secret) {
                        return refuse(&ws);
                    }
                    socket.ready = true;
                    ws.serialize_attachment(&socket)?;
                    let mac_id = socket.mac_id.clone().unwrap_or_default();
                    // The newest proven socket wins: after a network change the
                    // old one can look open long after the Mac left it.
                    for (other, attached) in self.sockets() {
                        if attached.id != socket.id && attached.mac_id.as_ref() == Some(&mac_id) {
                            self.drop_socket(&attached.id);
                            let _ = other.close(Some(1000), Some("replaced"));
                        }
                    }
                    console_log!("mac {} connected", &mac_id[..8]);
                    ws.send_with_str(protocol::READY)
                }
            }
        }

        async fn websocket_close(
            &self,
            ws: WebSocket,
            _code: usize,
            _reason: String,
            _was_clean: bool,
        ) -> Result<()> {
            self.closed(&ws)
        }

        async fn websocket_error(&self, ws: WebSocket, _error: Error) -> Result<()> {
            self.closed(&ws)
        }

        async fn alarm(&self) -> Result<Response> {
            let now = Date::now().as_millis();
            let mut open = false;
            for (ws, socket) in self.sockets() {
                if socket.ready {
                    let mac_id = socket.mac_id.as_deref().unwrap_or_default();
                    open |= ws.send_with_str(protocol::keepalive(mac_id)).is_ok();
                } else if now.saturating_sub(socket.opened) > PROOF_WAIT_MS {
                    let _ = ws.close(Some(1008), Some("refused"));
                } else {
                    open = true;
                }
            }
            if open {
                self.state.storage().set_alarm(KEEPALIVE).await?;
            }
            Response::ok("")
        }
    }

    impl Relay {
        fn allowed(&self) -> Option<BTreeSet<String>> {
            let text = self
                .env
                .secret("ALLOWED_MACS")
                .map(|value| value.to_string())
                .or_else(|_| self.env.var("ALLOWED_MACS").map(|value| value.to_string()))
                .ok()?;
            protocol::allowed_macs(&text)
        }

        fn sockets(&self) -> Vec<(WebSocket, Socket)> {
            self.state
                .get_websockets()
                .into_iter()
                .filter_map(|ws| {
                    let socket = ws.deserialize_attachment::<Socket>().ok()??;
                    Some((ws, socket))
                })
                .collect()
        }

        /// A new socket is never refused for room: it displaces at most one
        /// older socket still proving it is a Mac, never a proven Mac
        /// (`relay_admission`).
        async fn open(&self, req: &Request) -> Result<Response> {
            let upgrade = req.headers().get("Upgrade")?.unwrap_or_default();
            if !upgrade.eq_ignore_ascii_case("websocket") {
                return status(426);
            }
            // Cloudflare names the client here; a client cannot set it.
            let address = req
                .headers()
                .get("CF-Connecting-IP")?
                .filter(|address| !address.is_empty() && address.len() <= 64);
            let sockets = self.sockets();
            if address.as_ref().is_some_and(|address| {
                sockets
                    .iter()
                    .filter(|(_, socket)| !socket.ready && socket.address.as_ref() == Some(address))
                    .count()
                    >= HELD_PER_ADDRESS
            }) {
                return status(429);
            }
            let mut waiting: Vec<(WebSocket, Socket)> = sockets
                .into_iter()
                .filter(|(_, socket)| !socket.ready && !socket.closing)
                .collect();
            waiting.sort_by_key(|(_, socket)| socket.opened);
            let displaced = {
                let listed: Vec<Waiting<'_>> = waiting
                    .iter()
                    .map(|(_, socket)| Waiting {
                        address: socket.address.as_deref(),
                        named: socket.mac_id.is_some(),
                    })
                    .collect();
                relay_admission::displaced(&listed, address.as_deref())
            };
            if let Some(index) = displaced {
                let (ws, mut socket) = waiting.swap_remove(index);
                socket.closing = true;
                ws.serialize_attachment(&socket)?;
                let _ = ws.close(Some(1008), Some("replaced"));
            }
            let pair = WebSocketPair::new()?;
            self.state.accept_web_socket(&pair.server);
            pair.server.serialize_attachment(&Socket {
                id: uuid::Uuid::new_v4().to_string(),
                opened: Date::now().as_millis(),
                mac_id: None,
                secret: None,
                ready: false,
                address,
                closing: false,
            })?;
            if self.state.storage().get_alarm().await?.is_none() {
                self.state.storage().set_alarm(KEEPALIVE).await?;
            }
            Response::from_websocket(pair.client)
        }

        async fn forward(&self, mut req: Request, allowed: &BTreeSet<String>) -> Result<Response> {
            let declared = req
                .headers()
                .get("Content-Length")?
                .and_then(|length| length.parse::<usize>().ok());
            if declared.is_none_or(|length| length > MAX_PACKET_BYTES) {
                return status(413);
            }
            let body = req.bytes().await?;
            let Some(packet) = protocol::phone_packet(&body, allowed) else {
                return status(400);
            };
            let Some((ws, socket)) = self
                .sockets()
                .into_iter()
                .find(|(_, socket)| socket.ready && socket.mac_id.as_ref() == Some(&packet.mac_id))
            else {
                return status(503);
            };
            let id = packet.request_id.clone();
            let (reply, receiver) = oneshot::channel();
            {
                let mut pending = self.pending.borrow_mut();
                if pending.contains_key(&id) {
                    return status(409);
                }
                if pending
                    .values()
                    .filter(|p| p.mac_id == packet.mac_id)
                    .count()
                    >= PENDING_PER_MAC
                {
                    return status(429);
                }
                pending.insert(
                    id.clone(),
                    Pending {
                        socket: socket.id,
                        mac_id: packet.mac_id,
                        reply,
                    },
                );
            }
            let Ok(text) = String::from_utf8(body) else {
                self.pending.borrow_mut().remove(&id);
                return status(400);
            };
            if ws.send_with_str(text).is_err() {
                self.pending.borrow_mut().remove(&id);
                return status(503);
            }
            let outcome = select(receiver, Box::pin(Delay::from(REPLY_WAIT))).await;
            self.pending.borrow_mut().remove(&id);
            match outcome {
                Either::Left((Ok(text), _)) => {
                    let mut response = Response::ok(text)?;
                    response
                        .headers_mut()
                        .set("Content-Type", "application/json")?;
                    Ok(response)
                }
                // Once sent, a lost reply cannot establish that a write was unsent.
                _ => status(504),
            }
        }

        fn closed(&self, ws: &WebSocket) -> Result<()> {
            if let Some(socket) = ws.deserialize_attachment::<Socket>()? {
                self.drop_socket(&socket.id);
                if socket.ready {
                    let mac_id = socket.mac_id.unwrap_or_default();
                    console_log!("mac {} disconnected", &mac_id[..8.min(mac_id.len())]);
                }
            }
            Ok(())
        }

        /// Ends every request waiting on that socket; their phones get 504.
        fn drop_socket(&self, id: &str) {
            self.pending
                .borrow_mut()
                .retain(|_, waiting| waiting.socket != id);
        }
    }

    fn refuse(ws: &WebSocket) -> Result<()> {
        let _ = ws.close(Some(1008), Some("refused"));
        Ok(())
    }
}
