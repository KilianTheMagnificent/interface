// Public interoperability fixtures: known P-256 private scalars 1 and 2, never real identities.
import Foundation
import CryptoKit
func hex(_ bytes: Data) -> String { bytes.map { String(format: "%02x", $0) }.joined() }
let mac = try P256.KeyAgreement.PrivateKey(rawRepresentation: Data(repeating: 0, count: 31) + Data([1]))
let phone = try P256.KeyAgreement.PrivateKey(rawRepresentation: Data(repeating: 0, count: 31) + Data([2]))
let macId = hex(Data(SHA256.hash(data: mac.publicKey.x963Representation)))
let requestId = "12345678-1234-4234-8234-123456789abc"
func packet(sender: P256.KeyAgreement.PrivateKey, recipient: P256.KeyAgreement.PublicKey, direction: String, text: String) throws -> [String: Any] {
    let publicKey = hex(sender.publicKey.x963Representation)
    let info = Data("Interface connection v5|\(direction)|\(macId)|\(requestId)|\(publicKey)".utf8)
    var context = try HPKE.Sender(recipientKey: recipient, ciphersuite: .P256_SHA256_AES_GCM_256, info: info, authenticatedBy: sender)
    let ciphertext = try context.seal(Data(text.utf8), authenticating: info)
    return ["version":5,"macId":macId,"requestId":requestId,"senderKey":publicKey,"encapsulatedKey":hex(context.encapsulatedKey),"ciphertext":ciphertext.base64EncodedString()]
}
let value: [String: Any] = ["version":5,"macId":macId,"requestId":requestId,
                          "macPublic":hex(mac.publicKey.x963Representation),"phonePublic":hex(phone.publicKey.x963Representation),
                          "request":try packet(sender: phone, recipient: mac.publicKey, direction: "request", text: "Interface cross-language request"),
                          "response":try packet(sender: mac, recipient: phone.publicKey, direction: "response", text: "Interface cross-language response")]
print(String(decoding: try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]), as: UTF8.self))
