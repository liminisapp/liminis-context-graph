import Foundation

// MARK: - /v1/rerank request/response (issue #796)
//
// Cohere/Jina-style shape, the de-facto convention for this route name:
//
//   request : {"model": "...", "query": "...", "documents": ["...", ...], "top_n": N?}
//   response: {"object": "list", "model": "...",
//              "results": [{"index": 0, "relevance_score": 1.23}, ...]}
//
// `relevance_score` is the cross-encoder's RAW LOGIT (unbounded, higher = more relevant), not a
// probability: the caller (liminis-context-graph) squashes it itself so it can blend it with
// belief and recency signals. Results are ordered best-first; `index` refers to `documents`.

struct RerankRequest: Codable, Sendable {
    let query: String
    let documents: [String]
    let model: String?
    /// Return only the best `topN` results. Absent means all documents.
    let topN: Int?

    enum CodingKeys: String, CodingKey {
        case query
        case documents
        case model
        case topN = "top_n"
    }
}

struct RerankResult: Codable, Sendable, Equatable {
    let index: Int
    let relevanceScore: Double

    enum CodingKeys: String, CodingKey {
        case index
        case relevanceScore = "relevance_score"
    }
}

struct RerankResponse: Codable, Sendable {
    let object: String
    let model: String
    let results: [RerankResult]
}
