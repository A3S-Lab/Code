package code

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"testing"
)

type sdkRealFixture struct {
	SchemaVersion       uint                 `json:"schema_version"`
	ReportSchemaVersion uint                 `json:"report_schema_version"`
	FixtureID           string               `json:"fixture_id"`
	ChatModel           string               `json:"chat_model"`
	Embedding           sdkRealEmbedding     `json:"embedding"`
	Chunking            sdkRealChunking      `json:"chunking"`
	Rerank              sdkRealRerank        `json:"rerank"`
	Corpus              sdkRealCorpus        `json:"corpus"`
	Tasks               []sdkRealFixtureTask `json:"tasks"`
}

type sdkRealEmbedding struct {
	Provider  string `json:"provider"`
	Model     string `json:"model"`
	Revision  string `json:"revision"`
	Dimension uint   `json:"dimension"`
	QueryID   string `json:"query_id"`
}

type sdkRealChunking struct {
	Strategy     string   `json:"strategy"`
	TargetBytes  uint     `json:"target_bytes"`
	OverlapBytes uint     `json:"overlap_bytes"`
	Separators   []string `json:"separators"`
}

type sdkRealRerank struct {
	RequestedMode string `json:"requested_mode"`
	Algorithm     string `json:"algorithm"`
}

type sdkRealCorpus struct {
	DigestAlgorithm     string               `json:"digest_algorithm"`
	ExpectedDigest      string               `json:"expected_digest"`
	TextFileCount       uint                 `json:"text_file_count"`
	NonTextFileCount    uint                 `json:"non_text_file_count"`
	ExpectedChunkCount  uint                 `json:"expected_chunk_count"`
	UnrelatedFileCount  uint                 `json:"unrelated_file_count"`
	BoundaryFillerLines uint                 `json:"boundary_filler_lines"`
	SourceFiles         []sdkRealFixtureFile `json:"source_files"`
	NonTextFiles        []sdkRealFixtureFile `json:"non_text_files"`
}

type sdkRealFixtureFile struct {
	Path    string `json:"path"`
	Content string `json:"content"`
}

type sdkRealFixtureTask struct {
	Name               string `json:"name"`
	Query              string `json:"query"`
	ExpectedPath       string `json:"expected_path"`
	ExpectedIdentifier string `json:"expected_identifier"`
}

type sdkRealCorpusFile struct {
	path    string
	content []byte
	text    bool
}

func loadSDKRealFixture(t *testing.T) sdkRealFixture {
	t.Helper()
	_, source, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve Go fixture source path")
	}
	path := filepath.Join(
		filepath.Dir(source),
		"..",
		"evaluation",
		"workspace-retrieval-deepseek-v1.json",
	)
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var fixture sdkRealFixture
	if err := json.Unmarshal(data, &fixture); err != nil {
		t.Fatal(err)
	}
	if fixture.SchemaVersion != 1 || fixture.ReportSchemaVersion != 1 {
		t.Fatalf("unsupported fixture contract: %#v", fixture)
	}
	return fixture
}

func sdkRealCorpusFiles(fixture sdkRealFixture) []sdkRealCorpusFile {
	files := make([]sdkRealCorpusFile, 0, fixture.Corpus.TextFileCount+fixture.Corpus.NonTextFileCount)
	for _, file := range fixture.Corpus.SourceFiles {
		files = append(files, sdkRealCorpusFile{
			path: file.Path, content: []byte(file.Content), text: true,
		})
	}
	for index := uint(0); index < fixture.Corpus.UnrelatedFileCount; index++ {
		files = append(files, sdkRealCorpusFile{
			path: fmt.Sprintf("src/unrelated_%02d.rs", index),
			content: []byte(fmt.Sprintf(
				"pub fn unrelated_worker_%02d(value: usize) -> usize { value + %d }\n",
				index,
				index,
			)),
			text: true,
		})
	}
	var boundary strings.Builder
	for index := uint(0); index < fixture.Corpus.BoundaryFillerLines; index++ {
		fmt.Fprintf(&boundary, "// deterministic chunk-boundary filler %02d\n", index)
	}
	boundary.WriteString("pub const MAX_PENDING_EMBED_BATCHES: usize = 8;\n\n")
	boundary.WriteString("pub fn admits_batch(pending: usize) -> bool {\n")
	boundary.WriteString("    pending < MAX_PENDING_EMBED_BATCHES\n}\n")
	files = append(files, sdkRealCorpusFile{
		path: "src/embedding_admission.rs", content: []byte(boundary.String()), text: true,
	})
	for _, file := range fixture.Corpus.NonTextFiles {
		files = append(files, sdkRealCorpusFile{
			path: file.Path, content: []byte(file.Content), text: false,
		})
	}
	sort.Slice(files, func(left, right int) bool { return files[left].path < files[right].path })
	return files
}

func materializeSDKRealCorpus(t *testing.T, root string, fixture sdkRealFixture) string {
	t.Helper()
	hash := sha256.New()
	var textFiles uint
	var nonTextFiles uint
	for _, file := range sdkRealCorpusFiles(fixture) {
		destination := filepath.Join(root, filepath.FromSlash(file.path))
		if err := os.MkdirAll(filepath.Dir(destination), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(destination, file.content, 0o644); err != nil {
			t.Fatal(err)
		}
		_, _ = hash.Write([]byte(file.path))
		_, _ = hash.Write([]byte{0})
		_, _ = hash.Write(file.content)
		_, _ = hash.Write([]byte{0})
		if file.text {
			textFiles++
		} else {
			nonTextFiles++
		}
	}
	if textFiles != fixture.Corpus.TextFileCount || nonTextFiles != fixture.Corpus.NonTextFileCount {
		t.Fatalf("corpus counts = %d/%d", textFiles, nonTextFiles)
	}
	return hex.EncodeToString(hash.Sum(nil))
}

type sdkRealProviderCounters struct {
	sync.Mutex
	Requests         uint `json:"requests"`
	DocumentRequests uint `json:"documentRequests"`
	QueryRequests    uint `json:"queryRequests"`
	DocumentInputs   uint `json:"documentInputs"`
	QueryInputs      uint `json:"queryInputs"`
	InputBytes       uint `json:"inputBytes"`
	NonTextInputs    uint `json:"nonTextInputs"`
}

type sdkRealProviderMetric struct {
	Requests         uint `json:"requests"`
	DocumentRequests uint `json:"documentRequests"`
	QueryRequests    uint `json:"queryRequests"`
	DocumentInputs   uint `json:"documentInputs"`
	QueryInputs      uint `json:"queryInputs"`
	InputBytes       uint `json:"inputBytes"`
	NonTextInputs    uint `json:"nonTextInputs"`
}

func (counters *sdkRealProviderCounters) snapshot() sdkRealProviderMetric {
	counters.Lock()
	defer counters.Unlock()
	return sdkRealProviderMetric{
		Requests:         counters.Requests,
		DocumentRequests: counters.DocumentRequests,
		QueryRequests:    counters.QueryRequests,
		DocumentInputs:   counters.DocumentInputs,
		QueryInputs:      counters.QueryInputs,
		InputBytes:       counters.InputBytes,
		NonTextInputs:    counters.NonTextInputs,
	}
}

type sdkRealEmbeddingProvider struct {
	fixture  sdkRealFixture
	counters *sdkRealProviderCounters
}

func (provider *sdkRealEmbeddingProvider) Descriptor() EmbeddingProviderDescriptor {
	return EmbeddingProviderDescriptor{
		Provider:      provider.fixture.Embedding.Provider,
		Model:         provider.fixture.Embedding.Model,
		Revision:      provider.fixture.Embedding.Revision,
		Dimension:     provider.fixture.Embedding.Dimension,
		Normalization: EmbeddingNormalizationUnit,
	}
}

func (provider *sdkRealEmbeddingProvider) Embed(
	ctx context.Context,
	request EmbeddingBatchRequest,
) (EmbeddingBatchResponse, error) {
	if err := ctx.Err(); err != nil {
		return EmbeddingBatchResponse{}, err
	}
	query := len(request.Inputs) > 0
	documents := len(request.Inputs) > 0
	for _, input := range request.Inputs {
		query = query && input.ID == provider.fixture.Embedding.QueryID
		documents = documents && input.ID != provider.fixture.Embedding.QueryID
	}
	if !query && !documents {
		return EmbeddingBatchResponse{}, fmt.Errorf("document and query inputs share a batch")
	}
	provider.counters.Lock()
	provider.counters.Requests++
	if query {
		provider.counters.QueryRequests++
	} else {
		provider.counters.DocumentRequests++
	}
	for _, input := range request.Inputs {
		provider.counters.InputBytes += uint(len([]byte(input.Text)))
		if input.ID == provider.fixture.Embedding.QueryID {
			provider.counters.QueryInputs++
		} else {
			provider.counters.DocumentInputs++
		}
		if strings.Contains(input.Text, "NON_TEXT_ASSET_SENTINEL") {
			provider.counters.NonTextInputs++
		}
	}
	provider.counters.Unlock()
	vectors := make([]EmbeddingVector, 0, len(request.Inputs))
	for _, input := range request.Inputs {
		values, err := sdkRealVectorFor(provider.fixture, input.ID, input.Text)
		if err != nil {
			return EmbeddingBatchResponse{}, err
		}
		vectors = append(vectors, EmbeddingVector{ID: input.ID, Values: values})
	}
	return EmbeddingBatchResponse{Vectors: vectors}, nil
}

func sdkRealVectorFor(fixture sdkRealFixture, inputID, text string) ([]float32, error) {
	axis := -1
	if inputID == fixture.Embedding.QueryID {
		for index, task := range fixture.Tasks {
			if strings.TrimSpace(text) == task.Query {
				axis = index
				break
			}
		}
		if axis < 0 {
			return nil, fmt.Errorf("unexpected evaluation query %q", text)
		}
	} else {
		for index, task := range fixture.Tasks {
			if strings.Contains(text, task.ExpectedIdentifier) {
				axis = index
				break
			}
		}
		if axis < 0 {
			buckets := fixture.Embedding.Dimension - uint(len(fixture.Tasks))
			axis = len(fixture.Tasks) + int(sdkRealStableBucket(text, buckets))
		}
	}
	vector := make([]float32, fixture.Embedding.Dimension)
	vector[axis] = 1
	return vector, nil
}

func sdkRealStableBucket(text string, buckets uint) uint {
	var value uint32 = 2166136261
	for _, current := range []byte(text) {
		value ^= uint32(current)
		value *= 16777619
	}
	return uint(value) % buckets
}

func TestWorkspaceRetrievalRealFixtureContract(t *testing.T) {
	fixture := loadSDKRealFixture(t)
	digest := materializeSDKRealCorpus(t, t.TempDir(), fixture)
	if digest != fixture.Corpus.ExpectedDigest {
		t.Fatalf("corpus digest = %s, want %s", digest, fixture.Corpus.ExpectedDigest)
	}
}
