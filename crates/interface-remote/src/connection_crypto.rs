//! RFC 9180 authenticated HPKE. Session authority is checked separately by `RemoteHub`.
use base64::{Engine, engine::general_purpose::STANDARD};
use hpke::{
    Deserializable, Kem as KemTrait, OpModeR, OpModeS, Serializable, aead::AesGcm256,
    kdf::HkdfSha256, kem::DhP256HkdfSha256,
};
use interface_contracts::remote_connection::{EncryptedPacketV5, MAX_PACKET_BYTES, lowercase_hex};
use rand_core::{OsRng, TryRngCore};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use uuid::Uuid;

type Kem = DhP256HkdfSha256;
type Private = <Kem as KemTrait>::PrivateKey;
type Public = <Kem as KemTrait>::PublicKey;
pub const MAX_PLAINTEXT_BYTES: usize = 3 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct CryptoError;

pub struct ConnectionIdentity {
    private: Private,
    public: Public,
}
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        },
    )
}
/// Compares two secrets without stopping at the first differing byte. Only the
/// lengths, which are public here, can end it early.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let left = std::hint::black_box(left);
    let right = std::hint::black_box(right);
    if left.len() != right.len() {
        return false;
    }
    let difference = left.iter().zip(right).fold(0_u8, |difference, (&a, &b)| {
        std::hint::black_box(difference | (std::hint::black_box(a) ^ std::hint::black_box(b)))
    });
    std::hint::black_box(difference) == 0
}
/// # Errors
/// Refuses noncanonical or incorrectly sized hexadecimal encoding.
pub fn unhex(text: &str, size: usize) -> Result<Vec<u8>, CryptoError> {
    if !lowercase_hex(text, size) {
        return Err(CryptoError);
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| CryptoError)?;
            u8::from_str_radix(text, 16).map_err(|_| CryptoError)
        })
        .collect()
}
impl ConnectionIdentity {
    /// # Errors
    /// Fails when OS randomness is unavailable.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut seed = [0; 32];
        OsRng.try_fill_bytes(&mut seed).map_err(|_| CryptoError)?;
        let (private, public) = Kem::derive_keypair(&seed);
        seed.fill(0);
        Ok(Self { private, public })
    }
    /// # Errors
    /// Refuses invalid persisted private key material.
    pub fn from_private(value: &str) -> Result<Self, CryptoError> {
        let mut bytes = unhex(value, 32)?;
        let parsed = Private::from_bytes(&bytes).map_err(|_| CryptoError);
        bytes.fill(0);
        let private = parsed?;
        let public = Kem::sk_to_pk(&private);
        Ok(Self { private, public })
    }
    #[must_use]
    pub fn private_storage(&self) -> String {
        hex(&self.private.to_bytes())
    }
    #[must_use]
    pub fn public_key(&self) -> String {
        hex(&self.public.to_bytes())
    }
    #[must_use]
    pub fn mac_id(&self) -> String {
        hex(&Sha256::digest(self.public.to_bytes()))
    }

    /// Fresh HPKE context for every packet, with protocol/direction/identity/request binding.
    /// # Errors
    /// Refuses an invalid recipient, oversized content or encryption failure.
    pub fn seal(
        &self,
        recipient: &str,
        mac_id: &str,
        request_id: Uuid,
        direction: &str,
        plaintext: &[u8],
    ) -> Result<EncryptedPacketV5, CryptoError> {
        if plaintext.len() > MAX_PLAINTEXT_BYTES
            || !lowercase_hex(mac_id, 32)
            || !matches!(direction, "request" | "response" | "relayChallenge")
        {
            return Err(CryptoError);
        }
        let recipient = Public::from_bytes(&unhex(recipient, 65)?).map_err(|_| CryptoError)?;
        let sender_key = self.public_key();
        let info = context(mac_id, request_id, direction, &sender_key);
        let mode = OpModeS::Auth((self.private.clone(), self.public.clone()));
        let (enc, mut sender) = hpke::setup_sender::<AesGcm256, HkdfSha256, Kem, _>(
            &mode,
            &recipient,
            &info,
            &mut OsRng.unwrap_err(),
        )
        .map_err(|_| CryptoError)?;
        let ciphertext = sender.seal(plaintext, &info).map_err(|_| CryptoError)?;
        Ok(EncryptedPacketV5 {
            version: 5,
            mac_id: mac_id.into(),
            request_id: request_id.to_string(),
            sender_key,
            encapsulated_key: hex(&enc.to_bytes()),
            ciphertext: STANDARD.encode(ciphertext),
        })
    }

    /// # Errors
    /// Authenticates the packet before releasing any plaintext. Request authorization
    /// still requires an invitation or a `RemoteHub` credential/grant after this step.
    pub fn open(
        &self,
        packet: &EncryptedPacketV5,
        mac_id: &str,
        direction: &str,
        expected_sender: Option<&str>,
    ) -> Result<Vec<u8>, CryptoError> {
        let request_id = Uuid::parse_str(&packet.request_id).map_err(|_| CryptoError)?;
        if packet.version != 5
            || packet.mac_id != mac_id
            || !lowercase_hex(mac_id, 32)
            || packet.request_id != request_id.to_string()
            || packet.ciphertext.len() > MAX_PACKET_BYTES - 1024
            || expected_sender.is_some_and(|expected| expected != packet.sender_key)
            || !matches!(direction, "request" | "response" | "relayChallenge")
        {
            return Err(CryptoError);
        }
        let sender =
            Public::from_bytes(&unhex(&packet.sender_key, 65)?).map_err(|_| CryptoError)?;
        let enc = <Kem as KemTrait>::EncappedKey::from_bytes(&unhex(&packet.encapsulated_key, 65)?)
            .map_err(|_| CryptoError)?;
        let ciphertext = STANDARD
            .decode(&packet.ciphertext)
            .map_err(|_| CryptoError)?;
        if ciphertext.len() > MAX_PLAINTEXT_BYTES + 16 {
            return Err(CryptoError);
        }
        let info = context(mac_id, request_id, direction, &packet.sender_key);
        let mut receiver = hpke::setup_receiver::<AesGcm256, HkdfSha256, Kem>(
            &OpModeR::Auth(sender),
            &self.private,
            &enc,
            &info,
        )
        .map_err(|_| CryptoError)?;
        receiver.open(&ciphertext, &info).map_err(|_| CryptoError)
    }
}
fn context(mac_id: &str, request: Uuid, direction: &str, sender: &str) -> Vec<u8> {
    format!("Interface connection v5|{direction}|{mac_id}|{request}|{sender}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decrypts_apple_cryptokit_vectors_in_both_directions() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-connection-crypto-v5.json"
        ))
        .unwrap();
        let mac = ConnectionIdentity::from_private(&format!("{:064x}", 1)).unwrap();
        let phone = ConnectionIdentity::from_private(&format!("{:064x}", 2)).unwrap();
        for name in ["swift", "rust"] {
            let request: EncryptedPacketV5 =
                serde_json::from_value(vectors[name]["request"].clone()).unwrap();
            let response: EncryptedPacketV5 =
                serde_json::from_value(vectors[name]["response"].clone()).unwrap();
            assert_eq!(
                mac.open(
                    &request,
                    &mac.mac_id(),
                    "request",
                    Some(&phone.public_key())
                )
                .unwrap(),
                b"Interface cross-language request"
            );
            assert_eq!(
                phone
                    .open(
                        &response,
                        &mac.mac_id(),
                        "response",
                        Some(&mac.public_key())
                    )
                    .unwrap(),
                b"Interface cross-language response"
            );
        }
    }
    #[test]
    fn hpke_authenticates_both_directions_and_rejects_context_substitution() {
        let mac = ConnectionIdentity::generate().unwrap();
        let phone = ConnectionIdentity::generate().unwrap();
        let attacker = ConnectionIdentity::generate().unwrap();
        let id = Uuid::new_v4();
        let mac_id = mac.mac_id();
        let request = phone
            .seal(
                &mac.public_key(),
                &mac_id,
                id,
                "request",
                b"private request",
            )
            .unwrap();
        assert_eq!(
            mac.open(&request, &mac_id, "request", None).unwrap(),
            b"private request"
        );
        assert!(mac.open(&request, &mac_id, "response", None).is_err());
        assert!(attacker.open(&request, &mac_id, "request", None).is_err());
        let mut changed = request;
        changed.request_id = Uuid::new_v4().to_string();
        assert!(mac.open(&changed, &mac_id, "request", None).is_err());
        let response = mac
            .seal(
                &phone.public_key(),
                &mac_id,
                id,
                "response",
                b"private response",
            )
            .unwrap();
        assert_eq!(
            phone
                .open(&response, &mac_id, "response", Some(&mac.public_key()))
                .unwrap(),
            b"private response"
        );
        assert!(
            phone
                .open(&response, &mac_id, "response", Some(&attacker.public_key()))
                .is_err()
        );
    }

    #[test]
    fn secrets_match_only_byte_for_byte_at_the_same_length() {
        assert!(constant_time_eq(&[7; 32], &[7; 32]));
        assert!(constant_time_eq(&[], &[]));
        let mut changed = [7; 32];
        changed[31] = 6;
        assert!(!constant_time_eq(&[7; 32], &changed));
        assert!(!constant_time_eq(&[7; 32], &[7; 31]));
        assert!(!constant_time_eq(&[7; 31], &[7; 32]));
    }
}
