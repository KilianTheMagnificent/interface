import XCTest
@testable import interface_voice

final class VoiceBufferTests: XCTestCase {
    func testLevelMailboxCoalescesWhileTheOutputConsumerIsStalled() {
        let mailbox = LevelMailbox()
        XCTAssertNil(mailbox.take())
        // No consumer runs during this burst: publication must still complete
        // and retain only the final value, without a queue proportional to it.
        for sample in 0..<100_000 { mailbox.publish(Double(sample)) }
        XCTAssertEqual(mailbox.take(), 99_999)
        XCTAssertNil(mailbox.take())
        mailbox.publish(0)
        XCTAssertEqual(mailbox.take(), 0)
        for _ in 0..<100_000 { mailbox.reportConversionFailure() }
        XCTAssertTrue(mailbox.takeConversionFailure())
        XCTAssertFalse(mailbox.takeConversionFailure())
    }

    func testFramingAcrossUnicodeAndFinalLine() throws {
        var framer = VoiceLineFramer()
        var lines: [Data] = []
        let text = "{\"text\":\"界\"}\n{}"
        for byte in text.utf8 { lines += try framer.push(Data([byte])) }
        XCTAssertEqual(lines, [Data("{\"text\":\"界\"}".utf8)])
        XCTAssertEqual(framer.finish(), Data("{}".utf8))
        XCTAssertNil(framer.finish())
        XCTAssertThrowsError(try framer.push(Data(repeating: 65, count: VoiceLineFramer.maxBytes + 1)))
    }

    func testQueueBudgetsOrderAndRepeatedWraparound() {
        var queue = SpeechQueue()
        for round in 0..<10 {
            for index in 0..<SpeechQueue.maxCount {
                XCTAssertTrue(queue.append(SpeechRequest(id: "\(round):\(index)", text: "hello")))
            }
            XCTAssertFalse(queue.append(SpeechRequest(id: "overflow", text: "hello")))
            for index in 0..<SpeechQueue.maxCount {
                XCTAssertEqual(queue.popFirst()?.id, "\(round):\(index)")
            }
            XCTAssertTrue(queue.isEmpty)
            XCTAssertEqual(queue.bytes, 0)
        }
        XCTAssertFalse(queue.append(SpeechRequest(id: "huge", text: String(repeating: "界", count: SpeechQueue.maxTextBytes))))
        while queue.append(SpeechRequest(id: "x", text: String(repeating: "x", count: SpeechQueue.maxTextBytes))) {}
        XCTAssertLessThanOrEqual(queue.bytes, SpeechQueue.maxBytes)
        XCTAssertLessThan(queue.count, SpeechQueue.maxCount)
    }
}
