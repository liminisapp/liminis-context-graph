import Foundation

// MARK: - OpenAI-compatible embedding request/response types

struct EmbeddingRequest: Codable, Sendable {
    let input: EmbeddingInput
    let model: String?
    /// Optional role of the input: `"query"` or `"document"` (issue #790). Absent means the
    /// request behaves exactly as it always did. Kept as a raw string here so the handler can
    /// answer an unknown value with a 400 rather than a generic decode failure.
    var inputType: String? = nil

    enum CodingKeys: String, CodingKey {
        case input
        case model
        case inputType = "input_type"
    }

    enum EmbeddingInput: Codable, Sendable {
        case single(String)
        case batch([String])

        init(from decoder: any Decoder) throws {
            let container = try decoder.singleValueContainer()
            if let text = try? container.decode(String.self) {
                self = .single(text)
            } else {
                self = .batch(try container.decode([String].self))
            }
        }

        func encode(to encoder: any Encoder) throws {
            var container = encoder.singleValueContainer()
            switch self {
            case .single(let text): try container.encode(text)
            case .batch(let texts): try container.encode(texts)
            }
        }

        var texts: [String] {
            switch self {
            case .single(let text): return [text]
            case .batch(let texts): return texts
            }
        }
    }
}

struct EmbeddingData: Codable, Sendable {
    let object: String
    let embedding: [Double]
    let index: Int
}

struct EmbeddingUsage: Codable, Sendable {
    let promptTokens: Int
    let totalTokens: Int

    enum CodingKeys: String, CodingKey {
        case promptTokens = "prompt_tokens"
        case totalTokens = "total_tokens"
    }
}

struct EmbeddingResponse: Codable, Sendable {
    let object: String
    let data: [EmbeddingData]
    let model: String
    let usage: EmbeddingUsage
    /// Role-support advertisement (issue #790): present only when the loaded model applies
    /// per-role instructions on `input_type`. Omitted from the JSON when `nil`, so a model with
    /// no instructions serializes exactly as before.
    var instructionRoles: InstructionRoles? = nil

    enum CodingKeys: String, CodingKey {
        case object
        case data
        case model
        case usage
        case instructionRoles = "instruction_roles"
    }
}
