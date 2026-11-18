import Foundation
import Testing

@testable import LocalInference

// Issue #790: role-specific instructions, the reported model id and the wire shapes. All pure —
// no CoreML model or fixture is loaded, so these run anywhere the package builds.

@Suite("InstructionTable — which model applies which instruction")
struct InstructionTableTests {
    static let bgeQuery = "Represent this sentence for searching relevant passages: "

    @Test func bgeV15AppliesQueryInstructionOnly() {
        let roles = InstructionTable.roles(forModelId: "coreml-bge-base-en-v1.5")
        #expect(roles == InstructionRoles(query: Self.bgeQuery, document: ""))
    }

    @Test func bgeV15FamilyMatchesOtherSizes() {
        #expect(InstructionTable.roles(forModelId: "coreml-bge-large-en-v1.5")?.query == Self.bgeQuery)
    }

    @Test func unknownModelsHaveNoInstructions() {
        #expect(InstructionTable.roles(forModelId: "coreml-all-MiniLM-L6-v2") == nil)
        #expect(InstructionTable.roles(forModelId: "coreml-bge-m3") == nil)
        #expect(InstructionTable.roles(forModelId: "coreml-stub") == nil)
    }

    @Test func queryGetsInstructionDocumentDoesNot() {
        let roles = InstructionTable.roles(forModelId: "coreml-bge-base-en-v1.5")
        #expect(InstructionTable.apply(to: "find bread", inputType: .query, roles: roles) == Self.bgeQuery + "find bread")
        #expect(InstructionTable.apply(to: "a loaf", inputType: .document, roles: roles) == "a loaf")
    }

    @Test func noInputTypeLeavesTextUnchanged() {
        let roles = InstructionTable.roles(forModelId: "coreml-bge-base-en-v1.5")
        #expect(InstructionTable.apply(to: "find bread", inputType: nil, roles: roles) == "find bread")
    }

    @Test func modelWithoutInstructionsIgnoresInputType() {
        #expect(InstructionTable.apply(to: "find bread", inputType: .query, roles: nil) == "find bread")
    }

    /// REQ-12: an empty input must still reach the zero-vector path, never become a prefixed
    /// non-empty request.
    @Test func emptyInputIsNeverPrefixed() {
        let roles = InstructionTable.roles(forModelId: "coreml-bge-base-en-v1.5")
        #expect(InstructionTable.apply(to: "", inputType: .query, roles: roles) == "")
    }

    @Test func instructionIsAppliedExactlyOnce() {
        let roles = InstructionTable.roles(forModelId: "coreml-bge-base-en-v1.5")
        let once = InstructionTable.apply(to: "q", inputType: .query, roles: roles)
        #expect(once.components(separatedBy: Self.bgeQuery).count - 1 == 1)
    }
}

@Suite("EmbeddingModelId — the sidecar reports the real model, not a hard-coded name")
struct EmbeddingModelIdTests {
    /// FR-025/SC-011: the default model keeps reporting the exact string the sidecar used to
    /// hard-code, so existing workspaces are not flagged for a re-embed.
    @Test func defaultModelKeepsLegacyReportedId() {
        let url = URL(fileURLWithPath: "./bge-base-en-v1.5.mlpackage").standardized
        #expect(EmbeddingModelId.reportedId(modelURL: url) == "coreml-bge-base-en-v1.5")
    }

    @Test func compiledModelStemIsUsedToo() {
        let url = URL(fileURLWithPath: "/models/bge-base-en-v1.5.mlmodelc")
        #expect(EmbeddingModelId.reportedId(modelURL: url) == "coreml-bge-base-en-v1.5")
    }

    @Test func differentModelReportsItsOwnId() {
        let url = URL(fileURLWithPath: "/models/my-finetune.mlpackage")
        #expect(EmbeddingModelId.reportedId(modelURL: url) == "coreml-my-finetune")
        #expect(InstructionTable.roles(forModelId: EmbeddingModelId.reportedId(modelURL: url)) == nil)
    }

    @Test func explicitOverrideWins() {
        let url = URL(fileURLWithPath: "/models/renamed.mlpackage")
        #expect(EmbeddingModelId.reportedId(modelURL: url, override: "coreml-bge-base-en-v1.5") == "coreml-bge-base-en-v1.5")
    }

    @Test func emptyOverrideIsIgnored() {
        let url = URL(fileURLWithPath: "/models/bge-base-en-v1.5.mlpackage")
        #expect(EmbeddingModelId.reportedId(modelURL: url, override: "") == "coreml-bge-base-en-v1.5")
    }
}

@Suite("Embedding wire shapes — additive and backward compatible")
struct EmbeddingWireShapeTests {
    /// FR-023: a client sending only `{input, model}` still decodes, with no role.
    @Test func requestWithoutInputTypeDecodes() throws {
        let req = try JSONDecoder().decode(
            EmbeddingRequest.self, from: Data(#"{"input":"hello","model":"m"}"#.utf8))
        #expect(req.inputType == nil)
        #expect(req.input.texts == ["hello"])
    }

    @Test func requestDecodesInputTypeForSingleAndBatch() throws {
        let single = try JSONDecoder().decode(
            EmbeddingRequest.self, from: Data(#"{"input":"q","model":"m","input_type":"query"}"#.utf8))
        #expect(single.inputType == "query")
        let batch = try JSONDecoder().decode(
            EmbeddingRequest.self, from: Data(#"{"input":["a","b"],"input_type":"document"}"#.utf8))
        #expect(batch.inputType == "document")
        #expect(batch.input.texts == ["a", "b"])
    }

    @Test func inputTypeParsing() {
        #expect(EmbeddingInputType(rawValue: "query") == .query)
        #expect(EmbeddingInputType(rawValue: "document") == .document)
        #expect(EmbeddingInputType(rawValue: "passage") == nil)
    }

    private func encode(_ roles: InstructionRoles?) throws -> [String: Any] {
        let resp = EmbeddingResponse(
            object: "list",
            data: [EmbeddingData(object: "embedding", embedding: [0.5], index: 0)],
            model: "coreml-bge-base-en-v1.5",
            usage: EmbeddingUsage(promptTokens: 1, totalTokens: 1),
            instructionRoles: roles
        )
        let data = try JSONEncoder().encode(resp)
        return try JSONSerialization.jsonObject(with: data) as! [String: Any]
    }

    @Test func responseAdvertisesInstructionRoles() throws {
        let json = try encode(InstructionTable.roles(forModelId: "coreml-bge-base-en-v1.5"))
        let roles = try #require(json["instruction_roles"] as? [String: String])
        #expect(roles["query"] == InstructionTableTests.bgeQuery)
        #expect(roles["document"] == "")
        #expect(json["model"] as? String == "coreml-bge-base-en-v1.5")
    }

    /// A model with no instructions serializes exactly as before: no `instruction_roles` key.
    @Test func responseOmitsAdvertisementWhenModelHasNoInstructions() throws {
        let json = try encode(nil)
        #expect(json["instruction_roles"] == nil)
        #expect(Set(json.keys) == ["object", "data", "model", "usage"])
    }
}
