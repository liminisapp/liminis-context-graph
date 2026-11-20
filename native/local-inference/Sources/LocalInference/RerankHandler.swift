import Accelerate
import CoreML
import Foundation
import Hub
import Hummingbird
import Tokenizers

// MARK: - Load state

/// Where the rerank model is in its life cycle. Loading a large cross-encoder (mxbai-large) takes
/// minutes on a cold start, so the sidecar starts listening first and loads it in the background;
/// `/v1/rerank` answers a distinct "loading" 503 until the model is ready.
final class RerankLoadState: @unchecked Sendable {
    enum Phase: Sendable {
        case loading
        case ready(any RerankScorer)
        case failed(String)
    }

    private let lock = NSLock()
    private var current: Phase = .loading

    var phase: Phase {
        lock.lock()
        defer { lock.unlock() }
        return current
    }

    func setReady(_ scorer: any RerankScorer) { set(.ready(scorer)) }
    func setFailed(_ message: String) { set(.failed(message)) }

    private func set(_ phase: Phase) {
        lock.lock()
        current = phase
        lock.unlock()
    }
}

// MARK: - Compute units

/// Which Core ML compute units the rerank model runs on.
enum RerankComputeUnits {
    /// `LOCAL_INFERENCE_RERANK_COMPUTE_UNITS` (`cpuAndGPU`, `cpuAndNeuralEngine`, `cpuOnly`)
    /// wins. Unset: the Neural Engine for `bge-reranker-*` (measured fast there), the GPU for
    /// everything else — mxbai-rerank-large-v1 (DeBERTa-v3-large) takes about 3.8 s per batch of 8 on
    /// the Neural Engine and about 0.26 s on the GPU.
    static func resolve(_ value: String?, tokenizerModelId: String) throws -> MLComputeUnits {
        if let value {
            switch value.lowercased() {
            case "cpuandgpu": return .cpuAndGPU
            case "cpuandneuralengine": return .cpuAndNeuralEngine
            case "cpuonly": return .cpuOnly
            default:
                throw LocalInferenceError.rerankSchemaInvalid(
                    "unknown LOCAL_INFERENCE_RERANK_COMPUTE_UNITS '\(value)' (expected cpuAndGPU, cpuAndNeuralEngine or cpuOnly)")
            }
        }
        return tokenizerModelId.lowercased().contains("bge-reranker")
            ? .cpuAndNeuralEngine : .cpuAndGPU
    }
}

// MARK: - Scoring abstraction

/// Scores `(query, document)` pairs. `CoreMLRerankActor` is the production implementation; tests
/// inject a deterministic one so the route can be exercised without a CoreML model.
protocol RerankScorer: Sendable {
    /// The model id reported in responses.
    var modelId: String { get }
    /// One raw logit per document, in input order.
    func score(query: String, documents: [String]) async throws -> [Float]
}

// MARK: - Token layout and deterministic truncation

/// How a cross-encoder family lays out a (query, document) pair. Chosen by
/// `LOCAL_INFERENCE_RERANK_FAMILY`, which overrides the layout normally read from the tokenizer's
/// own `tokenizer.json` (see `fromTokenizerJSON`).
struct RerankTokenLayout: Sendable, Equatable {
    /// Token ids placed before the query.
    let prefix: [Int]
    /// Token ids between query and document.
    let middle: [Int]
    /// Token ids after the document.
    let suffix: [Int]
    let padId: Int
    /// Whether the model takes `token_type_ids` (BERT-style 0/1 segments). XLM-R does not.
    let usesTokenTypes: Bool

    /// `<s> q </s></s> d </s>`, pad = 1.
    static let xlmRoberta = RerankTokenLayout(
        prefix: [0], middle: [2, 2], suffix: [2], padId: 1, usesTokenTypes: false)
    /// `[CLS] q [SEP] d [SEP]`, pad = 0, with segment ids.
    static let bert = RerankTokenLayout(
        prefix: [101], middle: [102], suffix: [102], padId: 0, usesTokenTypes: true)

    static func named(_ name: String) -> RerankTokenLayout? {
        switch name.lowercased() {
        case "xlm-roberta", "xlmr", "roberta": return .xlmRoberta
        case "bert": return .bert
        default: return nil
        }
    }

    var specialTokenCount: Int { prefix.count + middle.count + suffix.count }

    /// The same layout with `usesTokenTypes` set from what the exported model actually accepts.
    func withTokenTypes(_ uses: Bool) -> RerankTokenLayout {
        RerankTokenLayout(
            prefix: prefix, middle: middle, suffix: suffix, padId: padId, usesTokenTypes: uses)
    }

    /// Derives the pair layout from a Hugging Face `tokenizer.json`, so no model family's special
    /// token ids are hard-coded: the tokenizer states how `(query, document)` is laid out.
    ///
    /// Reads the `post_processor`'s **pair** template (`TemplateProcessing`, as XLM-R, BERT and
    /// DeBERTa-v3 use), or the `cls`/`sep` of `RobertaProcessing` / `BertProcessing`. The special
    /// tokens before the first sequence form `prefix`, those between the two sequences `middle`,
    /// and those after the second `suffix`. The pad id is `padding.pad_id`, else the id of the
    /// added token named `[PAD]` / `<pad>`. `usesTokenTypes` is set by the caller from the model.
    static func fromTokenizerJSON(_ data: Data, usesTokenTypes: Bool = false) throws
        -> RerankTokenLayout
    {
        func fail(_ why: String) -> LocalInferenceError {
            .rerankSchemaInvalid("tokenizer.json: \(why)")
        }
        guard let root = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw fail("not a JSON object")
        }
        guard let post = root["post_processor"] as? [String: Any],
            let kind = post["type"] as? String
        else { throw fail("no post_processor") }

        var prefix: [Int] = []
        var middle: [Int] = []
        var suffix: [Int] = []

        switch kind {
        case "TemplateProcessing":
            guard let pair = post["pair"] as? [[String: Any]],
                let specials = post["special_tokens"] as? [String: Any]
            else { throw fail("TemplateProcessing without pair/special_tokens") }
            func ids(of name: String) throws -> [Int] {
                guard let entry = specials[name] as? [String: Any],
                    let ids = entry["ids"] as? [Int]
                else { throw fail("special token '\(name)' has no ids") }
                return ids
            }
            var sequencesSeen = 0
            for piece in pair {
                if piece["Sequence"] != nil {
                    sequencesSeen += 1
                } else if let tok = piece["SpecialToken"] as? [String: Any],
                    let name = tok["id"] as? String
                {
                    let tokenIds = try ids(of: name)
                    switch sequencesSeen {
                    case 0: prefix += tokenIds
                    case 1: middle += tokenIds
                    default: suffix += tokenIds
                    }
                }
            }
            guard sequencesSeen == 2 else { throw fail("pair template must have two sequences") }
        case "RobertaProcessing":
            guard let cls = (post["cls"] as? [Any])?.last as? Int,
                let sep = (post["sep"] as? [Any])?.last as? Int
            else { throw fail("RobertaProcessing without cls/sep") }
            (prefix, middle, suffix) = ([cls], [sep, sep], [sep])
        case "BertProcessing":
            guard let cls = (post["cls"] as? [Any])?.last as? Int,
                let sep = (post["sep"] as? [Any])?.last as? Int
            else { throw fail("BertProcessing without cls/sep") }
            (prefix, middle, suffix) = ([cls], [sep], [sep])
        default:
            throw fail("unsupported post_processor '\(kind)'")
        }

        var pad = (root["padding"] as? [String: Any])?["pad_id"] as? Int
        if pad == nil, let added = root["added_tokens"] as? [[String: Any]] {
            pad = added.first { ["[PAD]", "<pad>"].contains($0["content"] as? String ?? "") }?["id"]
                as? Int
        }
        guard let padId = pad else { throw fail("no pad token id") }
        return RerankTokenLayout(
            prefix: prefix, middle: middle, suffix: suffix, padId: padId,
            usesTokenTypes: usesTokenTypes)
    }
}

/// One tokenized, truncated, padded cross-encoder input row.
struct RerankRow: Sendable, Equatable {
    let inputIds: [Int]
    let attentionMask: [Int]
    let tokenTypeIds: [Int]
}

enum RerankInputBuilder {
    /// Builds one fixed-length row from already-tokenized (no special tokens) query and document.
    ///
    /// Truncation is deterministic and documented (issue #796): the **query** is kept up to half of
    /// the budget left after special tokens, the **document** gets everything else and is cut at
    /// its **tail** (head-biased: the start of a note is the best summary of it). So the same
    /// inputs always yield byte-identical rows, and a long query can never starve the document.
    static func build(
        queryIds: [Int],
        documentIds: [Int],
        layout: RerankTokenLayout,
        maxLength: Int
    ) -> RerankRow {
        let budget = max(2, maxLength - layout.specialTokenCount)
        let queryCap = max(1, budget / 2)
        let q = Array(queryIds.prefix(queryCap))
        let d = Array(documentIds.prefix(max(0, budget - q.count)))

        var ids = layout.prefix + q + layout.middle
        let segmentSplit = ids.count
        ids += d + layout.suffix
        let realCount = ids.count
        precondition(realCount <= maxLength, "rerank row exceeds maxLength")

        let pad = maxLength - realCount
        let inputIds = ids + Array(repeating: layout.padId, count: pad)
        let mask = Array(repeating: 1, count: realCount) + Array(repeating: 0, count: pad)
        let types: [Int] = layout.usesTokenTypes
            ? Array(repeating: 0, count: segmentSplit)
                + Array(repeating: 1, count: realCount - segmentSplit)
                + Array(repeating: 0, count: pad)
            : Array(repeating: 0, count: maxLength)
        return RerankRow(inputIds: inputIds, attentionMask: mask, tokenTypeIds: types)
    }
}

// MARK: - CoreML reranker actor

/// Cross-encoder reranker backed by a CoreML sequence-classification model.
///
/// The exported model has fixed `[B, L]` inputs (`input_ids`, `attention_mask`, optionally
/// `token_type_ids`) and a `logits` output of `[B, 1]` (or `[B]`). Documents are scored in groups
/// of `B`; a short final group is padded with dummy rows whose outputs are discarded. `B` and `L`
/// are read from the model, not assumed, so the batch size can be tuned at export time (see
/// `convert-rerank-model.py` and `benchmark/bench_rerank.py`).
actor CoreMLRerankActor: RerankScorer {
    static let outputFeatureName = "logits"

    private let model: MLModel
    private let tokenizer: any Tokenizer
    private let layout: RerankTokenLayout
    nonisolated let modelId: String
    let batchSize: Int
    let maxLength: Int

    /// Validates the model's inputs and output, returning `(batchSize, maxLength)`.
    static func validateSchema(_ model: MLModel, usesTokenTypes: Bool) throws -> (Int, Int) {
        let inputs = model.modelDescription.inputDescriptionsByName
        for name in ["input_ids", "attention_mask"] + (usesTokenTypes ? ["token_type_ids"] : []) {
            guard inputs[name]?.multiArrayConstraint != nil else {
                throw LocalInferenceError.rerankSchemaInvalid("missing multi-array input '\(name)'")
            }
        }
        guard let shape = inputs["input_ids"]?.multiArrayConstraint?.shape.map({ $0.intValue }),
            shape.count == 2, shape[0] >= 1, shape[1] >= 8
        else {
            throw LocalInferenceError.rerankSchemaInvalid("input_ids must have shape [batch, length]")
        }
        guard let out = model.modelDescription.outputDescriptionsByName[outputFeatureName]?
            .multiArrayConstraint
        else {
            throw LocalInferenceError.rerankSchemaInvalid("missing output '\(outputFeatureName)'")
        }
        let outShape = out.shape.map { $0.intValue }
        guard outShape == [shape[0], 1] || outShape == [shape[0]] else {
            throw LocalInferenceError.rerankSchemaInvalid(
                "output '\(outputFeatureName)' has shape \(outShape), expected [\(shape[0]), 1]")
        }
        guard [.float32, .float16].contains(out.dataType) else {
            throw LocalInferenceError.rerankSchemaInvalid("output dtype must be float16 or float32")
        }
        return (shape[0], shape[1])
    }

    init(
        modelURL: URL,
        tokenizerModelId: String,
        /// `nil`: derive the pair layout from the tokenizer's own `tokenizer.json`
        /// (`LOCAL_INFERENCE_RERANK_FAMILY` forces a built-in layout instead).
        layoutOverride: RerankTokenLayout? = nil,
        hubCachePath: String? = nil,
        mlmodelcCacheDir: URL? = nil,
        modelIdOverride: String? = nil,
        computeUnits: MLComputeUnits? = nil
    ) async throws {
        self.modelId =
            modelIdOverride ?? "coreml-\(modelURL.deletingPathExtension().lastPathComponent)"
        let config = MLModelConfiguration()
        config.computeUnits =
            try computeUnits
            ?? RerankComputeUnits.resolve(nil, tokenizerModelId: tokenizerModelId)
        print("local-inference: Rerank compute units: \(config.computeUnits)")
        // Reuse the embedding actor's compile-and-cache flow; the caller passes a directory
        // distinct from the embedding model's so the two compiled models never collide.
        let loadURL = try await CoreMLEmbeddingActor.resolveLoadURL(
            modelURL: modelURL, mlmodelcCacheDir: mlmodelcCacheDir,
            verifyComputeUnits: config.computeUnits, emitSetupEvents: false)
        self.model = try await MLModel.load(contentsOf: loadURL, configuration: config)
        // Segment ids are fed exactly when the exported model has a `token_type_ids` input.
        let usesTokenTypes =
            model.modelDescription.inputDescriptionsByName["token_type_ids"] != nil
        (self.batchSize, self.maxLength) = try Self.validateSchema(
            self.model, usesTokenTypes: usesTokenTypes)

        let hub: HubApi
        if let cachePath = hubCachePath {
            hub = HubApi(downloadBase: URL(fileURLWithPath: cachePath), useOfflineMode: true)
        } else {
            hub = .shared
        }
        self.tokenizer = try await AutoTokenizer.from(pretrained: tokenizerModelId, hubApi: hub)
        if let layoutOverride {
            self.layout = layoutOverride.withTokenTypes(usesTokenTypes)
        } else {
            let json = hub.localRepoLocation(Hub.Repo(id: tokenizerModelId))
                .appendingPathComponent("tokenizer.json")
            self.layout = try RerankTokenLayout.fromTokenizerJSON(
                try Data(contentsOf: json), usesTokenTypes: usesTokenTypes)
        }
        print("local-inference: Rerank pair layout \(layout.prefix) q \(layout.middle) d \(layout.suffix), pad \(layout.padId), token types \(layout.usesTokenTypes)")
        print("local-inference: CoreML rerank model loaded from \(modelURL.lastPathComponent) (batch \(batchSize), length \(maxLength))")
        print("local-inference: Rerank tokenizer: \(tokenizerModelId)\(hubCachePath != nil ? " (local cache)" : "")")
        // Fail loading (-> /v1/rerank answers 503) rather than serve a model whose batched
        // scores disagree with its solo scores.
        try await verifyBatchConsistency(scorer: self)
        print("local-inference: Rerank batch self-check passed")
    }

    func score(query: String, documents: [String]) throws -> [Float] {
        guard !documents.isEmpty else { return [] }
        let queryIds = tokenizer.encode(text: query, addSpecialTokens: false)
        let rows = documents.map { doc in
            RerankInputBuilder.build(
                queryIds: queryIds,
                documentIds: tokenizer.encode(text: doc, addSpecialTokens: false),
                layout: layout,
                maxLength: maxLength)
        }
        var scores: [Float] = []
        scores.reserveCapacity(rows.count)
        var start = 0
        while start < rows.count {
            let group = Array(rows[start..<min(start + batchSize, rows.count)])
            scores += try predict(group)
            start += batchSize
        }
        return scores
    }

    /// Runs one fixed-batch prediction over up to `batchSize` rows and returns one logit per row.
    private func predict(_ group: [RerankRow]) throws -> [Float] {
        let shape: [NSNumber] = [NSNumber(value: batchSize), NSNumber(value: maxLength)]
        let ids = try MLMultiArray(shape: shape, dataType: .int32)
        let mask = try MLMultiArray(shape: shape, dataType: .int32)
        let types = try MLMultiArray(shape: shape, dataType: .int32)
        let idsPtr = ids.dataPointer.bindMemory(to: Int32.self, capacity: batchSize * maxLength)
        let maskPtr = mask.dataPointer.bindMemory(to: Int32.self, capacity: batchSize * maxLength)
        let typesPtr = types.dataPointer.bindMemory(to: Int32.self, capacity: batchSize * maxLength)
        for b in 0..<batchSize {
            // Rows beyond the real group are dummies: a lone first token attended to, so the
            // attention softmax never sees an all-masked row.
            let row = b < group.count ? group[b] : nil
            for i in 0..<maxLength {
                let at = b * maxLength + i
                idsPtr[at] = Int32(row?.inputIds[i] ?? (i == 0 ? (layout.prefix.first ?? layout.padId) : layout.padId))
                maskPtr[at] = Int32(row?.attentionMask[i] ?? (i == 0 ? 1 : 0))
                typesPtr[at] = Int32(row?.tokenTypeIds[i] ?? 0)
            }
        }
        var features: [String: MLFeatureValue] = [
            "input_ids": MLFeatureValue(multiArray: ids),
            "attention_mask": MLFeatureValue(multiArray: mask),
        ]
        if layout.usesTokenTypes {
            features["token_type_ids"] = MLFeatureValue(multiArray: types)
        }
        let output = try model.prediction(from: MLDictionaryFeatureProvider(dictionary: features))
        guard let logits = output.featureValue(for: Self.outputFeatureName)?.multiArrayValue else {
            throw LocalInferenceError.rerankSchemaInvalid("missing output '\(Self.outputFeatureName)'")
        }
        return Self.readLogits(logits, count: group.count)
    }

    /// Reads the first `count` logits of a `[B, 1]` / `[B]` float16 or float32 output.
    ///
    /// Honours the array's **strides**: CoreML (the Neural Engine in particular) may return a
    /// `[B, 1]` output with padded rows (e.g. strides `[16, 1]`), so row `b` lives at element
    /// `b * strides[0]`, not at `b`. Reading by element index returns the right value for row 0
    /// and padding (exactly 0) for every other row.
    static func readLogits(_ array: MLMultiArray, count: Int) -> [Float] {
        let rowStride = array.strides.first?.intValue ?? 1
        let span = rowStride * max(0, count - 1) + 1
        switch array.dataType {
        case .float32:
            let p = array.dataPointer.bindMemory(to: Float.self, capacity: span)
            return (0..<count).map { p[$0 * rowStride] }
        case .float16:
            let p = array.dataPointer.bindMemory(to: Float16.self, capacity: span)
            return (0..<count).map { Float(p[$0 * rowStride]) }
        default:
            preconditionFailure("rerank logits dtype validated at startup")
        }
    }
}

// MARK: - Batch-consistency self-check

/// Scores `documents` in **one** call and again **one at a time**, and throws if any document's
/// batched score differs from its solo score by more than `tolerance` (absolute, or 5% relative,
/// whichever is larger). A correct model gives the same logit for a document wherever it sits in
/// a batch; a broken export or read-back (issue #796: only row 0 scored, the rest exactly 0)
/// does not. Runs at startup so a bad asset fails loading (the route then answers 503) instead of
/// silently mis-ranking every multi-candidate search.
func verifyBatchConsistency(
    scorer: any RerankScorer,
    query: String = "what is the capital of France",
    documents: [String] = [
        "Paris is the capital of France.",
        "Bananas are yellow and grow in bunches.",
        "Berlin is the capital of Germany.",
    ],
    tolerance: Float = 0.5
) async throws {
    let batched = try await scorer.score(query: query, documents: documents)
    guard batched.count == documents.count else {
        throw LocalInferenceError.rerankSchemaInvalid(
            "batch self-check: \(batched.count) scores for \(documents.count) documents")
    }
    for (i, doc) in documents.enumerated() {
        let solo = try await scorer.score(query: query, documents: [doc])
        guard let alone = solo.first else {
            throw LocalInferenceError.rerankSchemaInvalid("batch self-check: no solo score")
        }
        let allowed = max(tolerance, 0.05 * abs(alone))
        guard abs(batched[i] - alone) <= allowed else {
            throw LocalInferenceError.rerankSchemaInvalid(
                "batch self-check failed: document \(i) scores \(batched[i]) in a batch but \(alone) alone; the model or its output read-back is not batch-consistent")
        }
    }
}

// MARK: - Handler

/// Orders `scores` best-first (ties by lower index, so the result is deterministic) and keeps the
/// best `topN` when given.
func rankRerankResults(scores: [Float], topN: Int?) -> [RerankResult] {
    let ranked = scores.enumerated()
        .map { RerankResult(index: $0.offset, relevanceScore: Double($0.element)) }
        .sorted {
            $0.relevanceScore != $1.relevanceScore
                ? $0.relevanceScore > $1.relevanceScore : $0.index < $1.index
        }
    guard let n = topN, n >= 0 else { return ranked }
    return Array(ranked.prefix(n))
}

/// Upper bound on documents per request — a runaway caller must not pin the ANE.
let maxRerankDocuments = 256

func handleRerank(
    request: Request,
    context: BasicRequestContext,
    scorer: any RerankScorer
) async throws -> Response {
    let body: RerankRequest
    do {
        body = try await request.decode(as: RerankRequest.self, context: context)
    } catch is CancellationError {
        throw CancellationError()
    } catch let error as HTTPError where error.status == .contentTooLarge {
        // An oversized body is not a malformed one: report it as such (LCG caps each document, so
        // this indicates a caller sending far more text than any model reads).
        return makeErrorResponse(
            status: .contentTooLarge, message: "Rerank request body is too large")
    } catch {
        // Hummingbird surfaces both malformed JSON and a missing key as an `HTTPError`, not a
        // `DecodingError`, so any other decode failure here is the caller's: 400, never a 500.
        return makeErrorResponse(status: .badRequest, message: "Invalid rerank request body")
    }
    guard !body.query.isEmpty else {
        return makeErrorResponse(status: .badRequest, message: "query must not be empty")
    }
    guard !body.documents.isEmpty else {
        return makeErrorResponse(status: .badRequest, message: "documents must not be empty")
    }
    guard body.documents.count <= maxRerankDocuments else {
        return makeErrorResponse(
            status: .badRequest, message: "documents must not exceed \(maxRerankDocuments) entries")
    }
    if let n = body.topN, n < 0 {
        return makeErrorResponse(status: .badRequest, message: "top_n must be non-negative")
    }

    let scores = try await scorer.score(query: body.query, documents: body.documents)
    guard scores.count == body.documents.count, scores.allSatisfy({ $0.isFinite }) else {
        fputs("[local-inference] rerank produced \(scores.count) scores for \(body.documents.count) documents (or a non-finite score)\n", stderr)
        return makeErrorResponse(status: .internalServerError, message: "Rerank scoring failed")
    }

    let response = RerankResponse(
        object: "list",
        model: scorer.modelId,
        results: rankRerankResults(scores: scores, topN: body.topN))
    let jsonData = try JSONEncoder().encode(response)
    return Response(
        status: .ok,
        headers: [.contentType: "application/json"],
        body: .init(byteBuffer: .init(data: jsonData))
    )
}
