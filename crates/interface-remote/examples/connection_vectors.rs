//! Public test vectors only: P-256 private scalars 1 and 2 are deliberately not secrets.
use interface_remote::connection_crypto::ConnectionIdentity;
use uuid::Uuid;

fn main() {
    let mac = ConnectionIdentity::from_private(&format!("{:064x}", 1)).unwrap();
    let phone = ConnectionIdentity::from_private(&format!("{:064x}", 2)).unwrap();
    let request_id = Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap();
    let mac_id = mac.mac_id();
    let request = phone
        .seal(
            &mac.public_key(),
            &mac_id,
            request_id,
            "request",
            b"Interface cross-language request",
        )
        .unwrap();
    let response = mac
        .seal(
            &phone.public_key(),
            &mac_id,
            request_id,
            "response",
            b"Interface cross-language response",
        )
        .unwrap();
    println!(
        "{}",
        serde_json::json!({"version":5,"macId":mac_id,"requestId":request_id,
        "macPublic":mac.public_key(),"phonePublic":phone.public_key(),"request":request,"response":response})
    );
}
