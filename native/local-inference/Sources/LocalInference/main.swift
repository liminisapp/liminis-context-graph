import Foundation
import Hummingbird
import FoundationModels

// MARK: - Availability guard

guard #available(macOS 26, *) else {
    fputs("Error: local-inference requires macOS 26 or later\n", stderr)
    exit(1)
}

// MARK: - Configuration

let defaultSocketPath = "/tmp/liminis-inference.sock"
let socketPath = ProcessInfo.processInfo.environment["LOCAL_INFERENCE_SOCKET"]
    ?? defaultSocketPath

// MARK: - Mode selection
// LOCAL_INFERENCE_MODE selects which capability set the sidecar serves: `embeddings`,
// `completions`, or `both` (default). This gates the embedding-model guard/actor
// construction below and which routes buildRouter registers.

let mode: LocalInferenceMode
do {
    mode = try LocalInferenceMode.parse(ProcessInfo.processInfo.environment[LocalInferenceMode.envVarName])
} catch LocalInferenceMode.ParseError.invalidValue(let raw) {
    fputs("Error: invalid \(LocalInferenceMode.envVarName) value '\(raw)'. Must be one of: embeddings, completions, both.\n", stderr)
    exit(1)
}
print("local-inference: mode=\(mode.rawValue)")

// MARK: - CoreML model path validation (REQ-09)
// The sidecar exits with a clear error if the embedding model is missing.
// Silent fallback to NLEmbedding is forbidden — it would silently regress quality.
// Only enforced when the selected mode includes embeddings (FR-002).

let defaultModelPath = "./bge-base-en-v1.5.mlpackage"
let modelPath = URL(fileURLWithPath: ProcessInfo.processInfo.environment["LOCAL_INFERENCE_EMBEDDING_MODEL"]
    ?? defaultModelPath).standardized.path
let modelURL = URL(fileURLWithPath: modelPath)

// HuggingFace Hub local cache: structure must be <cachePath>/models/<org>/<model>/.
// If LOCAL_INFERENCE_HF_CACHE is set, tokenizer loads offline from that directory.
// If unset, swift-transformers downloads the tokenizer from HuggingFace Hub on first run.
// Canonicalize via URL.standardized to resolve any `..` components before passing to
// HubApi — workaround for swift-transformers v0.1.24 substring-match bug in snapshot().
let hubCachePath: String? = ProcessInfo.processInfo.environment["LOCAL_INFERENCE_HF_CACHE"]
    .map { URL(fileURLWithPath: $0).standardized.path }
let tokenizerModelId = ProcessInfo.processInfo.environment["LOCAL_INFERENCE_TOKENIZER_ID"]
    ?? "BAAI/bge-base-en-v1.5"

// Optional explicit model id reported to clients (issue #790). Defaults to
// `coreml-<model file stem>`; set this to pin the id if the model file is renamed, so an
// existing workspace's recorded embedding identity keeps matching.
let embeddingModelIdOverride: String? = ProcessInfo.processInfo.environment["LOCAL_INFERENCE_EMBEDDING_MODEL_ID"]

// First-launch cache for the compiled `.mlmodelc`. When set, the sidecar compiles
// the bundled `.mlpackage` once, persists the result under this directory, and
// loads from cache on subsequent launches. Owned by Electron; the sidecar creates
// the directory if missing and emits `[setup]` progress lines on stderr.
let mlmodelcCacheDir: URL? = ProcessInfo.processInfo.environment["LOCAL_INFERENCE_MLMODELC_CACHE"]
    .map { URL(fileURLWithPath: $0).standardized }

if mode.includesEmbeddings {
    guard FileManager.default.fileExists(atPath: modelPath) else {
        fputs("Error: CoreML embedding model not found at \(modelPath)\n", stderr)
        fputs("Run: uv run convert-embedding-model.py to generate it, then set LOCAL_INFERENCE_EMBEDDING_MODEL.\n", stderr)
        exit(1)
    }

    print("local-inference: embedding model path: \(modelPath)")
    print("local-inference: tokenizer: \(tokenizerModelId), HF cache: \(hubCachePath ?? "online (no LOCAL_INFERENCE_HF_CACHE set)")")
}

// MARK: - Foundation Models availability
// The guard moved out of startup — FoundationModelsAdapter checks at call time.
// The sidecar starts even if Apple Intelligence is disabled; only /v1/chat/completions
// is degraded (returns 503). The /v1/embeddings endpoint is unaffected.
// FoundationModelsAdapter() is constructed unconditionally (cheap: reads
// SystemLanguageModel.default.isAvailable) regardless of mode; only the status print
// below is mode-aware, so embeddings-only mode doesn't log a misleading "will return
// 503" line for a route that was never registered.

let fmAdapter = FoundationModelsAdapter()
if mode.includesCompletions {
    if fmAdapter.isAvailable {
        print("local-inference: Apple Foundation Models ready")
    } else {
        print("local-inference: Apple Foundation Models NOT available (Apple Intelligence disabled)")
        print("local-inference: /v1/chat/completions will return 503; /v1/embeddings unaffected")
    }
}

// MARK: - CoreML embedding actor initialization
// Wrap in do-catch so tokenizer/compile failures print a clear message instead
// of the default "Fatal error: Error raised at top level" Swift runtime message.
// Skipped entirely in completions-only mode (FR-004): no tokenizer load, no
// .mlmodelc compile, no LOCAL_INFERENCE_HF_CACHE requirement.

let embeddingActor: CoreMLEmbeddingActor?
if mode.includesEmbeddings {
    do {
        embeddingActor = try await CoreMLEmbeddingActor(
            modelURL: modelURL,
            tokenizerModelId: tokenizerModelId,
            hubCachePath: hubCachePath,
            mlmodelcCacheDir: mlmodelcCacheDir,
            modelIdOverride: embeddingModelIdOverride
        )
    } catch {
        // Surface the failure on the [setup] channel so the onboarding wizard can
        // show the actual reason, then print the legacy diagnostic for the log file.
        FileHandle.standardError.emitSetupEvent(stage: "setup_failed", message: "\(error)")
        fputs("Error: Failed to initialize CoreML embedding actor: \(error)\n", stderr)
        if hubCachePath != nil {
            fputs("Check that LOCAL_INFERENCE_HF_CACHE points to a valid HuggingFace cache directory.\n", stderr)
        } else {
            fputs("Set LOCAL_INFERENCE_HF_CACHE to a local HF cache, or ensure network access for online download.\n", stderr)
        }
        fputs("Also check that LOCAL_INFERENCE_EMBEDDING_MODEL is a valid .mlpackage or .mlmodelc path.\n", stderr)
        exit(1)
    }
} else {
    embeddingActor = nil
}

// MARK: - CoreML rerank actor initialization (issue #796)
// Optional: enabled iff LOCAL_INFERENCE_RERANK_MODEL is set, independent of LOCAL_INFERENCE_MODE.
// A load failure must NOT take the sidecar down — embeddings (and completions) are the product;
// the rerank stage is opt-in and degrades. So the failure is logged and `/v1/rerank` answers 503.

let env = ProcessInfo.processInfo.environment
// An empty or whitespace-only value counts as unset (a launcher template may leave it set to "").
func nonEmptyEnv(_ name: String) -> String? {
    guard let v = env[name]?.trimmingCharacters(in: .whitespacesAndNewlines), !v.isEmpty else { return nil }
    return v
}
let rerankModelPath: String? = nonEmptyEnv("LOCAL_INFERENCE_RERANK_MODEL")
    .map { URL(fileURLWithPath: $0).standardized.path }
// Loaded in the background so the sidecar listens (and serves embeddings) at once: a large
// cross-encoder takes minutes to load on a cold start. `/v1/rerank` answers a "loading" 503 until
// the model is ready, or the failure reason if it never is.
let rerankLoadState: RerankLoadState? = rerankModelPath == nil ? nil : RerankLoadState()
if let rerankModelPath, let rerankLoadState {
    let rerankTokenizerId = nonEmptyEnv("LOCAL_INFERENCE_RERANK_TOKENIZER_ID") ?? "mixedbread-ai/mxbai-rerank-large-v1"
    let rerankFamily = nonEmptyEnv("LOCAL_INFERENCE_RERANK_FAMILY")
    let rerankComputeUnits = nonEmptyEnv("LOCAL_INFERENCE_RERANK_COMPUTE_UNITS")
    let rerankModelId = nonEmptyEnv("LOCAL_INFERENCE_RERANK_MODEL_ID")
    let rerankCacheDir = mlmodelcCacheDir?.appendingPathComponent("rerank", isDirectory: true)
    let rerankHubCache = hubCachePath
    Task.detached {
        let started = Date()
        do {
            // Unset: the pair layout comes from the tokenizer's own tokenizer.json. The variable
            // remains an override for a tokenizer.json this reader cannot interpret.
            var layoutOverride: RerankTokenLayout? = nil
            if let familyName = rerankFamily {
                guard let named = RerankTokenLayout.named(familyName) else {
                    throw LocalInferenceError.rerankSchemaInvalid(
                        "unknown LOCAL_INFERENCE_RERANK_FAMILY '\(familyName)' (expected xlm-roberta or bert)")
                }
                layoutOverride = named
            }
            guard FileManager.default.fileExists(atPath: rerankModelPath) else {
                throw LocalInferenceError.rerankSchemaInvalid("rerank model not found at \(rerankModelPath)")
            }
            let units = try RerankComputeUnits.resolve(rerankComputeUnits, tokenizerModelId: rerankTokenizerId)
            let actor = try await CoreMLRerankActor(
                modelURL: URL(fileURLWithPath: rerankModelPath),
                tokenizerModelId: rerankTokenizerId,
                layoutOverride: layoutOverride,
                hubCachePath: rerankHubCache,
                // A subdirectory: the embedding model's compiled copy lives directly in the cache dir.
                mlmodelcCacheDir: rerankCacheDir,
                modelIdOverride: rerankModelId,
                computeUnits: units
            )
            rerankLoadState.setReady(actor)
            print("local-inference: rerank model ready after \(Int(Date().timeIntervalSince(started))) s")
        } catch {
            // A load failure must NOT take the sidecar down — embeddings (and completions) are the
            // product; the rerank stage is opt-in and degrades.
            fputs("Warning: rerank model failed to load: \(error); /v1/rerank will return 503\n", stderr)
            rerankLoadState.setFailed("\(error)")
        }
    }
    print("local-inference: loading the rerank model in the background; /v1/rerank returns 503 (loading) until it is ready")
} else {
    print("local-inference: rerank not configured (LOCAL_INFERENCE_RERANK_MODEL unset); /v1/rerank returns 404")
}

// Remove stale socket file so Hummingbird can bind cleanly on restart
try? FileManager.default.removeItem(atPath: socketPath)

// MARK: - Application

let app = Application(
    router: buildRouter(
        adapter: fmAdapter, embeddingActor: embeddingActor,
        rerankEnabled: rerankModelPath != nil, rerankLoadState: rerankLoadState, mode: mode),
    configuration: .init(address: .unixDomainSocket(path: socketPath))
)

print("local-inference: listening on unix:\(socketPath)")

try await app.runService()
