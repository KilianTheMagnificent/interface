//! The relay's side of the v5 connection, kept free of the Workers runtime so
//! it is tested natively. It speaks exactly what `interface-remote`'s Mac
//! (`http/direct.rs` `relay_session`) and phone expect from
//! `crates/interface-remote/src/relay.rs`; only ciphertext crosses it.
#[path = "../../../crates/interface-remote/src/connection_crypto.rs"]
#[allow(dead_code)]
mod connection_crypto;
/// Which waiting socket a new one displaces: the Mac's own rule, by path.
#[path = "../../../crates/interface-remote/src/relay_admission.rs"]
pub mod relay_admission;

use base64::{Engine, engine::general_purpose::STANDARD};
use connection_crypto::{ConnectionIdentity, constant_time_eq, hex, unhex};
use interface_contracts::remote_connection::{EncryptedPacketV5, MAX_PACKET_BYTES, lowercase_hex};
use rand_core::{OsRng, TryRngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

/// The Mac's first message. Same shape as `relay::RelayRegistration`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayRegistration {
    pub version: u8,
    pub public_key: String,
}
/// The Mac's answer to the sealed challenge. Same shape as `relay::RelayProof`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayProof {
    pub version: u8,
    pub proof: String,
}

/// What the Mac requires, value for value, before it counts the relay online.
pub const READY: &str = r#"{"version":5,"ready":true}"#;

/// The Macs this relay serves: public Mac identities (SHA-256 of the public
/// key, 64 lowercase hex), separated by commas or whitespace. `None` refuses
/// to serve anyone, as `relay_router` refuses to start without one.
#[must_use]
pub fn allowed_macs(text: &str) -> Option<BTreeSet<String>> {
    let macs: BTreeSet<String> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    (!macs.is_empty() && macs.iter().all(|id| lowercase_hex(id, 32))).then_some(macs)
}

pub struct Challenge {
    pub mac_id: String,
    /// Sent to the Mac; only the holder of its private key can open it.
    pub packet: EncryptedPacketV5,
    /// What the Mac must send back, kept until the proof arrives.
    pub secret: String,
}
/// Answers an allowed Mac's registration with a challenge sealed to its key.
#[must_use]
pub fn challenge(registration: &str, allowed: &BTreeSet<String>) -> Option<Challenge> {
    if registration.len() > 1024 {
        return None;
    }
    let registration: RelayRegistration = serde_json::from_str(registration).ok()?;
    let key = unhex(&registration.public_key, 65).ok()?;
    let mac_id = hex(&Sha256::digest(key));
    if registration.version != 5 || !allowed.contains(&mac_id) {
        return None;
    }
    let mut secret = [0; 32];
    OsRng.try_fill_bytes(&mut secret).ok()?;
    let packet = ConnectionIdentity::generate()
        .ok()?
        .seal(
            &registration.public_key,
            &mac_id,
            Uuid::new_v4(),
            "relayChallenge",
            &secret,
        )
        .ok()?;
    Some(Challenge {
        mac_id,
        packet,
        secret: hex(&secret),
    })
}
/// Whether the Mac's proof is the challenge it was sent.
#[must_use]
pub fn proves(proof: &str, secret: &str) -> bool {
    if proof.len() > 1024 {
        return false;
    }
    let (Ok(proof), Ok(secret)) = (serde_json::from_str::<RelayProof>(proof), unhex(secret, 32))
    else {
        return false;
    };
    proof.version == 5
        && STANDARD
            .decode(proof.proof)
            .is_ok_and(|bytes| constant_time_eq(&bytes, &secret))
}

/// A phone's packet the relay may carry: v5, for an allowed Mac, with a
/// request id. Its contents stay sealed to the Mac.
#[must_use]
pub fn phone_packet(body: &[u8], allowed: &BTreeSet<String>) -> Option<EncryptedPacketV5> {
    if body.len() > MAX_PACKET_BYTES {
        return None;
    }
    let packet: EncryptedPacketV5 = serde_json::from_slice(body).ok()?;
    (packet.version == 5
        && allowed.contains(&packet.mac_id)
        && Uuid::parse_str(&packet.request_id).is_ok())
    .then_some(packet)
}
/// A reply the registered Mac sent. Anything else ends its connection, as in
/// `relay::mac_socket`.
#[must_use]
pub fn mac_packet(text: &str, mac_id: &str) -> Option<EncryptedPacketV5> {
    if text.len() > MAX_PACKET_BYTES {
        return None;
    }
    let packet: EncryptedPacketV5 = serde_json::from_str(text).ok()?;
    (packet.version == 5 && packet.mac_id == mac_id).then_some(packet)
}

/// Workers cannot send WebSocket pings, and a Mac ends a relay connection
/// that stays silent for 50 s. This well-formed packet keeps it open: the Mac
/// parses it, fails to open it before any key work, and sends nothing back.
#[must_use]
pub fn keepalive(mac_id: &str) -> String {
    serde_json::to_string(&EncryptedPacketV5 {
        version: 5,
        mac_id: mac_id.into(),
        request_id: Uuid::nil().to_string(),
        sender_key: String::new(),
        encapsulated_key: String::new(),
        ciphertext: String::new(),
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration(identity: &ConnectionIdentity) -> String {
        serde_json::to_string(&RelayRegistration {
            version: 5,
            public_key: identity.public_key(),
        })
        .unwrap()
    }
    fn proof(bytes: &[u8]) -> String {
        serde_json::to_string(&RelayProof {
            version: 5,
            proof: STANDARD.encode(bytes),
        })
        .unwrap()
    }

    #[test]
    fn only_an_explicit_list_of_mac_identities_is_served() {
        let mac = "ab".repeat(32);
        let other = "cd".repeat(32);
        assert_eq!(
            allowed_macs(&format!(" {mac},\n{other} ")),
            Some(BTreeSet::from([mac.clone(), other]))
        );
        for refused in ["", " , ", "AB", &mac.to_uppercase(), &format!("{mac},xyz")] {
            assert_eq!(allowed_macs(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn only_the_mac_holding_the_private_key_can_answer_its_challenge() {
        let mac = ConnectionIdentity::generate().unwrap();
        let allowed = BTreeSet::from([mac.mac_id()]);
        let sent = challenge(&registration(&mac), &allowed).unwrap();
        assert_eq!(sent.mac_id, mac.mac_id());
        // Exactly what relay_session does with the challenge.
        let opened = mac
            .open(&sent.packet, &mac.mac_id(), "relayChallenge", None)
            .unwrap();
        assert!(proves(&proof(&opened), &sent.secret));
        assert!(!proves(&proof(&[0; 32]), &sent.secret));
        assert!(!proves(&proof(&opened[..31]), &sent.secret));
        // Knowing the public key is not enough to open it.
        let impostor = ConnectionIdentity::generate().unwrap();
        assert!(
            impostor
                .open(&sent.packet, &mac.mac_id(), "relayChallenge", None)
                .is_err()
        );
    }

    #[test]
    fn a_mac_outside_the_list_or_a_malformed_registration_gets_no_challenge() {
        let mac = ConnectionIdentity::generate().unwrap();
        let other = ConnectionIdentity::generate().unwrap();
        let allowed = BTreeSet::from([other.mac_id()]);
        assert!(challenge(&registration(&mac), &allowed).is_none());
        let allowed = BTreeSet::from([mac.mac_id()]);
        for refused in [
            format!(r#"{{"version":4,"publicKey":"{}"}}"#, mac.public_key()),
            format!(
                r#"{{"version":5,"publicKey":"{}","extra":1}}"#,
                mac.public_key()
            ),
            r#"{"version":5,"publicKey":"04"}"#.into(),
            format!(
                r#"{{"version":5,"publicKey":"{}"}}"#,
                mac.public_key().to_uppercase()
            ),
            format!(
                r#"{{"version":5,"publicKey":"{}{}"}}"#,
                mac.public_key(),
                " ".repeat(1024)
            ),
        ] {
            assert!(challenge(&refused, &allowed).is_none(), "{refused}");
        }
    }

    #[test]
    fn phone_packets_are_carried_only_for_an_allowed_mac() {
        let mac = ConnectionIdentity::generate().unwrap();
        let phone = ConnectionIdentity::generate().unwrap();
        let allowed = BTreeSet::from([mac.mac_id()]);
        let packet = phone
            .seal(
                &mac.public_key(),
                &mac.mac_id(),
                Uuid::new_v4(),
                "request",
                b"x",
            )
            .unwrap();
        let body = serde_json::to_vec(&packet).unwrap();
        assert!(phone_packet(&body, &allowed).is_some());
        assert!(phone_packet(&body, &BTreeSet::from(["ab".repeat(32)])).is_none());
        let mut changed = packet;
        changed.request_id = "not-a-uuid".into();
        assert!(phone_packet(&serde_json::to_vec(&changed).unwrap(), &allowed).is_none());
        assert!(phone_packet(b"{}", &allowed).is_none());
    }

    #[test]
    fn replies_must_come_from_the_registered_mac() {
        let mac = ConnectionIdentity::generate().unwrap();
        let reply = serde_json::to_string(
            &mac.seal(
                &mac.public_key(),
                &mac.mac_id(),
                Uuid::new_v4(),
                "response",
                b"x",
            )
            .unwrap(),
        )
        .unwrap();
        assert!(mac_packet(&reply, &mac.mac_id()).is_some());
        assert!(mac_packet(&reply, &"ab".repeat(32)).is_none());
        assert!(mac_packet(READY, &mac.mac_id()).is_none());
    }

    #[test]
    fn keepalive_is_a_packet_the_mac_parses_but_cannot_open() {
        let mac = ConnectionIdentity::generate().unwrap();
        let text = keepalive(&mac.mac_id());
        // relay_session ends the connection on text that is not a packet.
        let packet: EncryptedPacketV5 = serde_json::from_str(&text).unwrap();
        assert!(mac.open(&packet, &mac.mac_id(), "request", None).is_err());
        assert!(mac_packet(&text, &mac.mac_id()).is_some());
    }

    #[test]
    fn ready_is_the_exact_value_the_mac_waits_for() {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(READY).unwrap(),
            serde_json::json!({"version":5,"ready":true})
        );
    }
}
