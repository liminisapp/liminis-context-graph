// Generated from the real tokenizer.json of each model (post_processor, padding and the pad
// added token only; the vocabulary is omitted) plus `transformers` output for
// tokenizer(query, document): query = "who owns Marlin Core?",
// document = "Marlin Core is owned by Northwind Holdings. It was acquired in 2021.".
// Regenerate with the recipe in native/local-inference/README.md (Reranker, tokenizer fixtures).

struct RerankTokenizerFixture {
    let model: String
    let tokenizerJSON: String
    let queryIds: [Int]
    let documentIds: [Int]
    let transformersPairIds: [Int]
    let transformersTokenTypeIds: [Int]?
}

let rerankTokenizerFixtures: [RerankTokenizerFixture] = [
    RerankTokenizerFixture(
        model: "BAAI/bge-reranker-base",
        tokenizerJSON: #"{"post_processor":{"type":"TemplateProcessing","single":[{"SpecialToken":{"id":"<s>","type_id":0}},{"Sequence":{"id":"A","type_id":0}},{"SpecialToken":{"id":"</s>","type_id":0}}],"pair":[{"SpecialToken":{"id":"<s>","type_id":0}},{"Sequence":{"id":"A","type_id":0}},{"SpecialToken":{"id":"</s>","type_id":0}},{"SpecialToken":{"id":"</s>","type_id":0}},{"Sequence":{"id":"B","type_id":0}},{"SpecialToken":{"id":"</s>","type_id":0}}],"special_tokens":{"</s>":{"id":"</s>","ids":[2],"tokens":["</s>"]},"<s>":{"id":"<s>","ids":[0],"tokens":["<s>"]}}},"padding":null,"added_tokens":[{"id":1,"content":"<pad>","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true}]}"#,
        queryIds: [2750, 10002, 7, 1784, 2397, 31365, 32],
        documentIds: [1784, 2397, 31365, 83, 6, 157272, 390, 23924, 50777, 73436, 7, 5, 1650, 509, 163629, 71, 23, 64371, 5],
        transformersPairIds: [0, 2750, 10002, 7, 1784, 2397, 31365, 32, 2, 2, 1784, 2397, 31365, 83, 6, 157272, 390, 23924, 50777, 73436, 7, 5, 1650, 509, 163629, 71, 23, 64371, 5, 2],
        transformersTokenTypeIds: nil),
    RerankTokenizerFixture(
        model: "mixedbread-ai/mxbai-rerank-large-v1",
        tokenizerJSON: #"{"post_processor":{"type":"TemplateProcessing","single":[{"SpecialToken":{"id":"[CLS]","type_id":0}},{"Sequence":{"id":"A","type_id":0}},{"SpecialToken":{"id":"[SEP]","type_id":0}}],"pair":[{"SpecialToken":{"id":"[CLS]","type_id":0}},{"Sequence":{"id":"A","type_id":0}},{"SpecialToken":{"id":"[SEP]","type_id":0}},{"Sequence":{"id":"B","type_id":1}},{"SpecialToken":{"id":"[SEP]","type_id":1}}],"special_tokens":{"[CLS]":{"id":"[CLS]","ids":[1],"tokens":["[CLS]"]},"[SEP]":{"id":"[SEP]","ids":[2],"tokens":["[SEP]"]}}},"padding":{"strategy":"BatchLongest","direction":"Right","pad_to_multiple_of":null,"pad_id":0,"pad_type_id":0,"pad_token":"[PAD]"},"added_tokens":[{"id":0,"content":"[PAD]","single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special":true}]}"#,
        queryIds: [328, 8028, 47203, 6708, 302],
        documentIds: [47203, 6708, 269, 2702, 293, 975, 20039, 13397, 260, 325, 284, 4394, 267, 16789, 260],
        transformersPairIds: [1, 328, 8028, 47203, 6708, 302, 2, 47203, 6708, 269, 2702, 293, 975, 20039, 13397, 260, 325, 284, 4394, 267, 16789, 260, 2],
        transformersTokenTypeIds: [0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]),
]
