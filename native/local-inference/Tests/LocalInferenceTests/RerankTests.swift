import CoreML
import Foundation
import HTTPTypes
import Hummingbird
import HummingbirdTesting
import NIOCore
import Testing

@testable import LocalInference

// MARK: - Test scorer

/// Scores a document by its length, so ordering is predictable without a CoreML model.
struct LengthScorer: RerankScorer {
    var modelId: String { "stub-reranker" }
    func score(query: String, documents: [String]) async throws -> [Float] {
        documents.map { Float($0.count) }
    }
}

/// Returns the wrong number of scores — a misbehaving backend.
struct ShortScorer: RerankScorer {
    var modelId: String { "stub-reranker" }
    func score(query: String, documents: [String]) async throws -> [Float] { [1.0] }
}

private func makeApp(
    reranker: (any RerankScorer)?, enabled: Bool, mode: LocalInferenceMode = .both
) -> Application<RouterResponder<BasicRequestContext>> {
    Application(
        responder: buildRouter(
            adapter: MockInferenceAdapter(), reranker: reranker, rerankEnabled: enabled, mode: mode
        ).buildResponder())
}

private func postRerank<T: Sendable>(
    _ client: some TestClientProtocol,
    body: String,
    _ closure: @escaping @Sendable (TestResponse) async throws -> T
) async throws -> T {
    try await client.execute(
        uri: "/v1/rerank",
        method: .post,
        headers: [.contentType: "application/json"],
        body: ByteBuffer(string: body),
        testCallback: closure
    )
}

// MARK: - Route behaviour

@Suite("Rerank route")
struct RerankRouteTests {
    @Test func notConfiguredIs404() async throws {
        let app = makeApp(reranker: nil, enabled: false)
        try await app.test(.router) { client in
            try await postRerank(client, body: #"{"query":"q","documents":["a"]}"#) { r in
                #expect(r.status == .notFound)
            }
        }
    }

    @Test func configuredButUnloadedIs503() async throws {
        let app = makeApp(reranker: nil, enabled: true)
        try await app.test(.router) { client in
            try await postRerank(client, body: #"{"query":"q","documents":["a"]}"#) { r in
                #expect(r.status == .serviceUnavailable)
            }
        }
    }

    /// Rerank is independent of LOCAL_INFERENCE_MODE: it is served in every mode when configured.
    @Test func servedInEveryMode() async throws {
        for mode in [LocalInferenceMode.both, .embeddings, .completions] {
            let app = makeApp(reranker: LengthScorer(), enabled: true, mode: mode)
            try await app.test(.router) { client in
                try await postRerank(client, body: #"{"query":"q","documents":["a","bbb"]}"#) { r in
                    #expect(r.status == .ok, "mode \(mode.rawValue)")
                }
            }
        }
    }

    @Test func returnsResultsBestFirstWithOriginalIndex() async throws {
        let app = makeApp(reranker: LengthScorer(), enabled: true)
        try await app.test(.router) { client in
            try await postRerank(
                client, body: #"{"model":"m","query":"q","documents":["aa","a","aaaa"]}"#
            ) { r in
                #expect(r.status == .ok)
                let resp = try JSONDecoder().decode(
                    RerankResponse.self, from: Data(r.body.readableBytesView))
                #expect(resp.model == "stub-reranker")
                #expect(resp.results.map(\.index) == [2, 0, 1])
                #expect(resp.results.map(\.relevanceScore) == [4.0, 2.0, 1.0])
            }
        }
    }

    @Test func topNLimitsResults() async throws {
        let app = makeApp(reranker: LengthScorer(), enabled: true)
        try await app.test(.router) { client in
            try await postRerank(
                client, body: #"{"query":"q","documents":["aa","a","aaaa"],"top_n":1}"#
            ) { r in
                let resp = try JSONDecoder().decode(
                    RerankResponse.self, from: Data(r.body.readableBytesView))
                #expect(resp.results.map(\.index) == [2])
            }
        }
    }

    @Test func rejectsBadRequests() async throws {
        let app = makeApp(reranker: LengthScorer(), enabled: true)
        let tooMany = (0...maxRerankDocuments).map { _ in "\"d\"" }.joined(separator: ",")
        let bodies = [
            "not json",
            #"{"documents":["a"]}"#,
            #"{"query":"","documents":["a"]}"#,
            #"{"query":"q","documents":[]}"#,
            #"{"query":"q","documents":["a"],"top_n":-1}"#,
            #"{"query":"q","documents":[\#(tooMany)]}"#,
        ]
        try await app.test(.router) { client in
            for body in bodies {
                try await postRerank(client, body: body) { r in
                    #expect(r.status == .badRequest, "body: \(body.prefix(40))")
                }
            }
        }
    }

    @Test func aBackendReturningTheWrongNumberOfScoresIs500NotAPartialResult() async throws {
        let app = makeApp(reranker: ShortScorer(), enabled: true)
        try await app.test(.router) { client in
            try await postRerank(client, body: #"{"query":"q","documents":["a","b"]}"#) { r in
                #expect(r.status == .internalServerError)
            }
        }
    }

    @Test func otherRoutesAreUnaffected() async throws {
        let app = makeApp(reranker: LengthScorer(), enabled: true)
        try await app.test(.router) { client in
            try await client.execute(uri: "/health", method: .get) { r in
                #expect(r.status == .ok)
            }
        }
    }
}

// MARK: - Ranking

@Suite("Rerank ranking")
struct RerankRankingTests {
    @Test func tiesBreakByLowerIndex() {
        let r = rankRerankResults(scores: [1, 3, 3, 2], topN: nil)
        #expect(r.map(\.index) == [1, 2, 3, 0])
    }

    @Test func topNZeroIsEmpty() {
        #expect(rankRerankResults(scores: [1, 2], topN: 0).isEmpty)
    }
}

// MARK: - Deterministic truncation

@Suite("Rerank input builder")
struct RerankInputBuilderTests {
    let xlmr = RerankTokenLayout.xlmRoberta
    let bert = RerankTokenLayout.bert

    @Test func xlmRobertaLayoutAndPadding() {
        let row = RerankInputBuilder.build(
            queryIds: [10, 11], documentIds: [20, 21, 22], layout: xlmr, maxLength: 16)
        #expect(Array(row.inputIds.prefix(10)) == [0, 10, 11, 2, 2, 20, 21, 22, 2, 1])
        #expect(row.inputIds.count == 16)
        #expect(row.attentionMask == Array(repeating: 1, count: 9) + Array(repeating: 0, count: 7))
        #expect(row.tokenTypeIds == Array(repeating: 0, count: 16))
    }

    @Test func bertLayoutSegmentsQueryAndDocument() {
        let row = RerankInputBuilder.build(
            queryIds: [10], documentIds: [20, 21], layout: bert, maxLength: 10)
        #expect(Array(row.inputIds.prefix(6)) == [101, 10, 102, 20, 21, 102])
        #expect(Array(row.tokenTypeIds.prefix(7)) == [0, 0, 0, 1, 1, 1, 0])
        #expect(row.inputIds.suffix(4).allSatisfy { $0 == 0 })
    }

    @Test func longDocumentIsCutAtTheTailAndNeverExceedsMaxLength() {
        let doc = Array(100..<2_100)
        let row = RerankInputBuilder.build(
            queryIds: [10, 11, 12], documentIds: doc, layout: xlmr, maxLength: 64)
        #expect(row.inputIds.count == 64)
        #expect(row.attentionMask.allSatisfy { $0 == 1 })  // full: nothing padded
        #expect(row.inputIds.last == 2)  // closing separator survives truncation
        // Head-biased: the document keeps its first tokens.
        let docStart = row.inputIds.firstIndex(of: 100)!
        #expect(Array(row.inputIds[docStart..<docStart + 5]) == [100, 101, 102, 103, 104])
        #expect(!row.inputIds.contains(2_099))
    }

    @Test func aLongQueryCannotStarveTheDocument() {
        let query = Array(1_000..<2_000)
        let row = RerankInputBuilder.build(
            queryIds: query, documentIds: [20, 21, 22, 23], layout: xlmr, maxLength: 64)
        #expect(row.inputIds.count == 64)
        #expect(row.inputIds.contains(20), "document must keep room")
        // Query is capped at half of the post-special-token budget (64 - 4) / 2 = 30.
        #expect(row.inputIds.filter { $0 >= 1_000 }.count == 30)
    }

    @Test func isDeterministic() {
        let a = RerankInputBuilder.build(
            queryIds: [1, 2, 3], documentIds: Array(0..<500), layout: bert, maxLength: 128)
        let b = RerankInputBuilder.build(
            queryIds: [1, 2, 3], documentIds: Array(0..<500), layout: bert, maxLength: 128)
        #expect(a == b)
    }

    @Test func familyNamesResolve() {
        #expect(RerankTokenLayout.named("xlm-roberta") == .xlmRoberta)
        #expect(RerankTokenLayout.named("BERT") == .bert)
        #expect(RerankTokenLayout.named("gpt") == nil)
    }
}

// MARK: - Batch consistency (issue #796: only row 0 of a batch was scored)

/// Scores each document by its length, wherever it sits in the request.
private struct ConsistentScorer: RerankScorer {
    var modelId: String { "consistent" }
    func score(query: String, documents: [String]) async throws -> [Float] {
        documents.map { Float($0.count) - 20 }
    }
}

/// Reproduces the reported bug: row 0 of a batch is scored, every other row reads back as 0.
private struct OnlyFirstRowScorer: RerankScorer {
    var modelId: String { "broken" }
    func score(query: String, documents: [String]) async throws -> [Float] {
        documents.enumerated().map { $0.offset == 0 ? Float($0.element.count) - 20 : 0 }
    }
}

struct RerankBatchConsistencyTests {
    @Test func aBatchConsistentScorerPasses() async throws {
        try await verifyBatchConsistency(scorer: ConsistentScorer())
    }

    @Test func aScorerThatOnlyScoresTheFirstRowIsRejected() async throws {
        await #expect(throws: LocalInferenceError.self) {
            try await verifyBatchConsistency(scorer: OnlyFirstRowScorer())
        }
    }

    @Test func aWrongScoreCountIsRejected() async throws {
        await #expect(throws: LocalInferenceError.self) {
            try await verifyBatchConsistency(scorer: ShortScorer())
        }
    }

    /// A `[B, 1]` output whose rows are padded (strides `[16, 1]`) must be read row by row, not
    /// by element index. Reading `[0..<B]` would return row 0 then padding zeros.
    @Test func logitsAreReadThroughTheRowStride() throws {
        let rows = 4
        let rowStride = 16
        let buffer = UnsafeMutablePointer<Float>.allocate(capacity: rows * rowStride)
        buffer.initialize(repeating: 0, count: rows * rowStride)
        defer { buffer.deallocate() }
        let want: [Float] = [8.5, -9.75, 3.25, -1.5]
        for (b, v) in want.enumerated() { buffer[b * rowStride] = v }
        let array = try MLMultiArray(
            dataPointer: buffer, shape: [NSNumber(value: rows), 1], dataType: .float32,
            strides: [NSNumber(value: rowStride), 1], deallocator: nil)
        #expect(CoreMLRerankActor.readLogits(array, count: rows) == want)
        #expect(CoreMLRerankActor.readLogits(array, count: 2) == [8.5, -9.75])
    }

    @Test func float16LogitsAreReadThroughTheRowStride() throws {
        let rows = 3
        let rowStride = 16
        let buffer = UnsafeMutablePointer<Float16>.allocate(capacity: rows * rowStride)
        buffer.initialize(repeating: 0, count: rows * rowStride)
        defer { buffer.deallocate() }
        let want: [Float16] = [8.75, -9.5, 2.0]
        for (b, v) in want.enumerated() { buffer[b * rowStride] = v }
        let array = try MLMultiArray(
            dataPointer: buffer, shape: [NSNumber(value: rows), 1], dataType: .float16,
            strides: [NSNumber(value: rowStride), 1], deallocator: nil)
        #expect(CoreMLRerankActor.readLogits(array, count: rows) == want.map { Float($0) })
    }

    /// A contiguous `[B]` output (stride 1) still reads element by element.
    @Test func contiguousLogitsStillReadInOrder() throws {
        let array = try MLMultiArray(shape: [3], dataType: .float32)
        for (i, v) in [1.5, -2.5, 4.0].enumerated() { array[i] = NSNumber(value: v) }
        #expect(CoreMLRerankActor.readLogits(array, count: 3) == [1.5, -2.5, 4.0])
    }
}

// MARK: - Tokenizer-derived pair layout (real tokenizer.json fragments)

@Suite("Rerank pair layout from tokenizer.json")
struct RerankTokenizerLayoutTests {
    @Test func layoutsDeriveFromTheRealTokenizerJSONOfEachSupportedModel() throws {
        let layouts = try rerankTokenizerFixtures.map {
            try RerankTokenLayout.fromTokenizerJSON(Data($0.tokenizerJSON.utf8))
        }
        // bge-reranker-base (XLM-R): `<s> q </s></s> d </s>`, pad 1 — from `added_tokens`, since
        // its tokenizer.json has no `padding` entry.
        #expect(layouts[0] == .xlmRoberta)
        // mxbai-rerank-large-v1 (DeBERTa-v3): `[CLS] q [SEP] d [SEP]`, pad 0 — read from the
        // tokenizer, not hard-coded (BERT's ids would be 101/102).
        #expect(layouts[1] == RerankTokenLayout(
            prefix: [1], middle: [2], suffix: [2], padId: 0, usesTokenTypes: false))
    }

    @Test func rowsMatchTheTransformersPairEncoding() throws {
        for fx in rerankTokenizerFixtures {
            let layout = try RerankTokenLayout.fromTokenizerJSON(
                Data(fx.tokenizerJSON.utf8), usesTokenTypes: fx.transformersTokenTypeIds != nil)
            let row = RerankInputBuilder.build(
                queryIds: fx.queryIds, documentIds: fx.documentIds, layout: layout, maxLength: 64)
            let real = fx.transformersPairIds.count
            #expect(Array(row.inputIds.prefix(real)) == fx.transformersPairIds, "\(fx.model)")
            #expect(row.attentionMask == Array(repeating: 1, count: real)
                + Array(repeating: 0, count: 64 - real), "\(fx.model)")
            #expect(row.inputIds.dropFirst(real).allSatisfy { $0 == layout.padId }, "\(fx.model)")
            if let types = fx.transformersTokenTypeIds {
                #expect(Array(row.tokenTypeIds.prefix(real)) == types, "\(fx.model)")
            }
        }
    }

    @Test func robertaAndBertProcessorsAreReadToo() throws {
        let roberta = #"{"post_processor":{"type":"RobertaProcessing","sep":["</s>",2],"cls":["<s>",0]},"padding":{"pad_id":1}}"#
        let bert = #"{"post_processor":{"type":"BertProcessing","sep":["[SEP]",102],"cls":["[CLS]",101]},"padding":{"pad_id":0}}"#
        #expect(try RerankTokenLayout.fromTokenizerJSON(Data(roberta.utf8)) == .xlmRoberta)
        #expect(try RerankTokenLayout.fromTokenizerJSON(Data(bert.utf8), usesTokenTypes: true) == .bert)
    }

    @Test func anUnreadableTokenizerJSONIsRejectedNotGuessed() {
        for bad in ["[]", #"{}"#, #"{"post_processor":{"type":"ByteLevel"}}"#,
            #"{"post_processor":{"type":"RobertaProcessing","sep":["</s>",2],"cls":["<s>",0]}}"#]
        {
            #expect(throws: (any Error).self) {
                try RerankTokenLayout.fromTokenizerJSON(Data(bad.utf8))
            }
        }
    }
}


// MARK: - Load state and compute units (issue #796)

@Suite("Rerank load state and compute units")
struct RerankLoadStateTests {
    private func app(_ state: RerankLoadState) -> Application<RouterResponder<BasicRequestContext>> {
        Application(
            responder: buildRouter(
                adapter: MockInferenceAdapter(), rerankEnabled: true, rerankLoadState: state
            ).buildResponder())
    }

    @Test func loadingAnswersADistinct503ThenServesOnceReady() async throws {
        let state = RerankLoadState()
        let application = app(state)
        try await application.test(.router) { client in
            try await postRerank(client, body: #"{"query":"q","documents":["a"]}"#) { r in
                #expect(r.status == .serviceUnavailable)
                #expect(String(buffer: r.body).contains("still loading"))
            }
            state.setReady(LengthScorer())
            try await postRerank(client, body: #"{"query":"q","documents":["a"]}"#) { r in
                #expect(r.status == .ok)
            }
        }
    }

    @Test func failedLoadReportsTheReason() async throws {
        let state = RerankLoadState()
        state.setFailed("boom")
        try await app(state).test(.router) { client in
            try await postRerank(client, body: #"{"query":"q","documents":["a"]}"#) { r in
                #expect(r.status == .serviceUnavailable)
                #expect(String(buffer: r.body).contains("boom"))
            }
        }
    }

    @Test func computeUnitsDefaultPerModelAndAcceptAnOverride() throws {
        #expect(try RerankComputeUnits.resolve(nil, tokenizerModelId: "mixedbread-ai/mxbai-rerank-large-v1") == .cpuAndGPU)
        #expect(try RerankComputeUnits.resolve(nil, tokenizerModelId: "BAAI/bge-reranker-base") == .cpuAndNeuralEngine)
        #expect(try RerankComputeUnits.resolve("cpuOnly", tokenizerModelId: "BAAI/bge-reranker-base") == .cpuOnly)
        #expect(try RerankComputeUnits.resolve("cpuAndNeuralEngine", tokenizerModelId: "x") == .cpuAndNeuralEngine)
        #expect(throws: LocalInferenceError.self) {
            try RerankComputeUnits.resolve("gpu", tokenizerModelId: "x")
        }
    }
}
