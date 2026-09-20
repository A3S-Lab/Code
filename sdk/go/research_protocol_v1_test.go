package code

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestProvenanceReceiptEnvelopeSurvivesReopen(t *testing.T) {
	digest := "sha256:" + strings.Repeat("a", 64)
	payload := json.RawMessage(`{"receipt_digest":"` + digest + `"}`)
	envelope := ResearchWireEnvelopeV1{
		Schema:  ResearchProtocolSchemaV1,
		Version: ResearchProtocolVersionV1,
		Kind:    ResearchWireResearchProvenanceReceipt,
		Payload: payload,
	}
	encoded, err := json.Marshal(envelope)
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "receipt.json")
	if err := os.WriteFile(path, encoded, 0o644); err != nil {
		t.Fatal(err)
	}
	reopened, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := DecodeResearchWireEnvelopeV1(reopened)
	if err != nil {
		t.Fatal(err)
	}
	if decoded.Kind != ResearchWireResearchProvenanceReceipt {
		t.Fatalf("kind = %q", decoded.Kind)
	}
	if !bytes.Equal(decoded.Payload, payload) {
		t.Fatalf("payload changed: got %s", decoded.Payload)
	}
	if !bytes.Contains(decoded.Payload, []byte(digest)) {
		t.Fatalf("receipt digest dropped: %s", decoded.Payload)
	}
}
