import Foundation

// MARK: - Role-specific embedding instructions (issue #790)
//
// Asymmetric embedding models are trained with a role-specific instruction: BGE v1.5 wants a
// query prefix and nothing on documents. This file holds the *pure* pieces — which model gets
// which instruction, how an instruction is applied, and which model id the sidecar reports — so
// they can be unit-tested without loading a CoreML model or its stub fixtures.
//
// The sidecar stays an OpenAI-compatible embedder: a client that sends only `{input, model}`
// gets exactly the vectors it always did. The instruction is applied only when the request
// carries the optional `input_type` AND the loaded model has an instruction for that role.
// LCG decides whether to send `input_type` (profile `embedder`); merely advertising the roles
// never changes any vector.

/// The optional `input_type` request field (Voyage/Cohere style).
enum EmbeddingInputType: String, Sendable {
    case query
    case document
}

/// The per-role instruction strings a model applies, advertised in the `/v1/embeddings` response
/// as `instruction_roles` so LCG can learn them at probe time — including the document-side one,
/// which LCG records in the workspace's embedding identity.
struct InstructionRoles: Codable, Sendable, Equatable {
    let query: String
    let document: String
}

enum InstructionTable {
    /// BGE-v1.5's documented query instruction (the trailing space is part of the string).
    static let bgeQueryInstruction = "Represent this sentence for searching relevant passages: "

    /// The instructions for the model identified by `modelId`, or `nil` when the model has none
    /// (no advertisement, and `input_type` is accepted but ignored). Matches the BGE `en-v1.5`
    /// family (e.g. `coreml-bge-base-en-v1.5`): query instruction only, documents unchanged.
    static func roles(forModelId modelId: String) -> InstructionRoles? {
        let id = modelId.lowercased()
        guard id.contains("bge"), id.contains("en-v1.5") else { return nil }
        return InstructionRoles(query: bgeQueryInstruction, document: "")
    }

    /// Applies the instruction for `inputType` to `text`. Returns `text` unchanged when no
    /// `input_type` was sent, the model has no instructions, or `text` is empty — an empty input
    /// must still reach the zero-vector path (REQ-12), never become a non-empty prefixed request.
    static func apply(to text: String, inputType: EmbeddingInputType?, roles: InstructionRoles?) -> String {
        guard let inputType, let roles, !text.isEmpty else { return text }
        switch inputType {
        case .query: return roles.query + text
        case .document: return roles.document + text
        }
    }
}

enum EmbeddingModelId {
    /// The model id the sidecar reports in every `/v1/embeddings` response.
    ///
    /// - An explicit non-empty `override` (`LOCAL_INFERENCE_EMBEDDING_MODEL_ID`) wins verbatim.
    /// - Otherwise it is `coreml-<file stem of the loaded model>`, with a trailing `.mlpackage`
    ///   or `.mlmodelc` stripped. The default model `./bge-base-en-v1.5.mlpackage` therefore
    ///   still reports `coreml-bge-base-en-v1.5` — byte for byte what the sidecar used to
    ///   hard-code — so existing workspaces are not flagged for a re-embed (FR-025). Loading a
    ///   different model via `LOCAL_INFERENCE_EMBEDDING_MODEL` now reports that model's own name.
    ///   Renaming the model file changes the reported id; use the override to pin it.
    static func reportedId(modelURL: URL, override: String? = nil) -> String {
        if let override, !override.isEmpty { return override }
        let stem = modelURL.deletingPathExtension().lastPathComponent
        return "coreml-\(stem)"
    }
}
