package code

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

type modelGenerationPoolHealthFixture struct {
	SchemaVersion          uint     `json:"schema_version"`
	ReportSchemaVersion    uint     `json:"report_schema_version"`
	FixtureID              string   `json:"fixture_id"`
	SampleLimit            uint     `json:"sample_limit"`
	MaxConcurrency         uint64   `json:"max_concurrency"`
	RequiredSnapshotFields []string `json:"required_snapshot_fields"`
	RequiredIdentityFields []string `json:"required_identity_fields"`
	AggregateFields        []string `json:"aggregate_fields"`
	ForbiddenFields        []string `json:"forbidden_fields"`
}

func loadModelGenerationPoolHealthFixture(t *testing.T) modelGenerationPoolHealthFixture {
	t.Helper()
	_, source, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve Go fixture source path")
	}
	data, err := os.ReadFile(filepath.Join(filepath.Dir(source), "..", "evaluation", "model-generation-pool-health-v1.json"))
	if err != nil {
		t.Fatal(err)
	}
	var fixture modelGenerationPoolHealthFixture
	if err := json.Unmarshal(data, &fixture); err != nil {
		t.Fatal(err)
	}
	if fixture.SchemaVersion != 1 || fixture.ReportSchemaVersion != 1 ||
		fixture.FixtureID == "" || fixture.SampleLimit == 0 || fixture.MaxConcurrency == 0 {
		t.Fatalf("invalid model-generation pool health fixture: %#v", fixture)
	}
	return fixture
}

func TestModelGenerationPoolHealthFixtureWithRuntime(t *testing.T) {
	fixture := loadModelGenerationPoolHealthFixture(t)
	digest := sha256.Sum256([]byte("model-generation-pool-health-fixture"))
	identity := ExecutionIdentityV1{
		Schema: "a3s.code.execution-identity.v1",
		Domain: "a3s.code.model-generation-pool.identity.v1",
		Digest: "sha256:" + hex.EncodeToString(digest[:]),
	}
	runtime := &fakeRuntime{
		request: func(_ context.Context, operation string, _ map[string]any) (any, error) {
			if operation != "session_model_generation_pool_health" {
				t.Fatalf("unexpected operation %q", operation)
			}
			return &ModelGenerationPoolHealthSnapshot{
				Pool:                ModelGenerationPool{Identity: identity, MaxConcurrency: 1},
				LocalMaxConcurrency: 1,
				LocalAvailable:      1,
				Scheduler: &TaskSchedulerQuotaHealthSnapshot{
					Identity:  identity,
					MaxActive: 1,
					Observed:  true,
				},
			}, nil
		},
	}
	session := testSession(runtime)
	var aggregate modelGenerationPoolHealthAggregate
	for sample := uint(0); sample < fixture.SampleLimit && sample < 3; sample++ {
		health, err := session.ModelGenerationPoolHealth(context.Background())
		if err != nil || health == nil {
			t.Fatalf("pool health = %#v, %v", health, err)
		}
		validateModelGenerationPoolHealthFixture(t, health, fixture)
		aggregate.observe(health)
	}
	if aggregate.SampleCount == 0 || aggregate.SampleCount > uint64(fixture.SampleLimit) {
		t.Fatalf("aggregate sample count = %d", aggregate.SampleCount)
	}
	encoded, err := json.Marshal(aggregate)
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]any
	if err := json.Unmarshal(encoded, &fields); err != nil {
		t.Fatal(err)
	}
	assertFixtureFields(t, fields, fixture.AggregateFields)
	assertNoForbiddenFields(t, fields, fixture.ForbiddenFields)
}

// TestModelGenerationPoolHealthFixtureWithRustBridge exercises the same
// fixture through the real Go JSONL bridge when a matching bridge binary is
// available. It is opt-in so ordinary SDK unit tests stay hermetic.
func TestModelGenerationPoolHealthFixtureWithRustBridge(t *testing.T) {
	binary := os.Getenv("A3S_CODE_GO_BRIDGE_TEST_BINARY")
	if binary == "" {
		t.Skip("set A3S_CODE_GO_BRIDGE_TEST_BINARY to run the Rust bridge fixture")
	}
	fixture := loadModelGenerationPoolHealthFixture(t)
	ctx := context.Background()
	agent, err := Create(ctx, `
default_model = "openai/fixture-model"
providers "openai" {
  apiKey = "fixture-key"
  baseUrl = "https://fixture.invalid/v1"
  models "fixture-model" {
    name = "Fixture Model"
  }
}
`, WithLocalRuntimeOptions(WithBridgePath(binary)))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = agent.Close(context.Background()) })
	session, err := agent.Session(ctx, t.TempDir(), &SessionOptions{SessionID: "pool-health-fixture"})
	if err != nil {
		t.Fatal(err)
	}
	health, err := session.ModelGenerationPoolHealth(ctx)
	if err != nil || health == nil {
		t.Fatalf("real bridge pool health = %#v, %v", health, err)
	}
	validateModelGenerationPoolHealthFixture(t, health, fixture)
	if err := session.Close(ctx); err != nil {
		t.Fatal(err)
	}
}

type modelGenerationPoolHealthAggregate struct {
	SampleCount         uint64 `json:"sampleCount"`
	MaxLocalReserved    uint64 `json:"maxLocalReserved"`
	MaxSchedulerActive  uint64 `json:"maxSchedulerActive"`
	MaxSchedulerPending uint64 `json:"maxSchedulerPending"`
	Admitted            uint64 `json:"admitted"`
	Released            uint64 `json:"released"`
	Cancelled           uint64 `json:"cancelled"`
	Rejected            uint64 `json:"rejected"`
}

func (aggregate *modelGenerationPoolHealthAggregate) observe(health *ModelGenerationPoolHealthSnapshot) {
	aggregate.SampleCount++
	if health.LocalReserved > aggregate.MaxLocalReserved {
		aggregate.MaxLocalReserved = health.LocalReserved
	}
	if health.Scheduler == nil {
		return
	}
	if health.Scheduler.Active > aggregate.MaxSchedulerActive {
		aggregate.MaxSchedulerActive = health.Scheduler.Active
	}
	if health.Scheduler.Pending > aggregate.MaxSchedulerPending {
		aggregate.MaxSchedulerPending = health.Scheduler.Pending
	}
	if health.Scheduler.Admitted > aggregate.Admitted {
		aggregate.Admitted = health.Scheduler.Admitted
	}
	if health.Scheduler.Released > aggregate.Released {
		aggregate.Released = health.Scheduler.Released
	}
	if health.Scheduler.Cancelled > aggregate.Cancelled {
		aggregate.Cancelled = health.Scheduler.Cancelled
	}
	if health.Scheduler.Rejected > aggregate.Rejected {
		aggregate.Rejected = health.Scheduler.Rejected
	}
}

func validateModelGenerationPoolHealthFixture(t *testing.T, health *ModelGenerationPoolHealthSnapshot, fixture modelGenerationPoolHealthFixture) {
	t.Helper()
	encoded, err := json.Marshal(health)
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]any
	if err := json.Unmarshal(encoded, &fields); err != nil {
		t.Fatal(err)
	}
	assertFixtureFields(t, fields, fixture.RequiredSnapshotFields)
	assertNoForbiddenFields(t, fields, fixture.ForbiddenFields)
	if health.Pool.MaxConcurrency == 0 || health.Pool.MaxConcurrency > fixture.MaxConcurrency {
		t.Fatalf("pool max concurrency = %d", health.Pool.MaxConcurrency)
	}
	if health.LocalReserved+health.LocalAvailable != health.LocalMaxConcurrency ||
		health.LocalMaxConcurrency > health.Pool.MaxConcurrency {
		t.Fatalf("invalid local capacity: %#v", health)
	}
	if health.Pool.Identity.Schema == "" || health.Pool.Identity.Domain == "" || health.Pool.Identity.Digest == "" {
		t.Fatalf("incomplete pool identity: %#v", health.Pool.Identity)
	}
	if health.Scheduler != nil {
		if health.Scheduler.Identity != health.Pool.Identity {
			t.Fatalf("scheduler identity drift: %#v vs %#v", health.Scheduler.Identity, health.Pool.Identity)
		}
		if health.Scheduler.MaxActive != health.Pool.MaxConcurrency ||
			health.Scheduler.Active > health.Scheduler.MaxActive ||
			health.Scheduler.Pending > health.Scheduler.MaxActive {
			t.Fatalf("invalid scheduler capacity: %#v", health.Scheduler)
		}
	}
	identityFields, ok := fields["pool"].(map[string]any)["identity"].(map[string]any)
	if !ok {
		t.Fatal("pool identity is not an object")
	}
	assertFixtureFields(t, identityFields, fixture.RequiredIdentityFields)
}

func assertFixtureFields(t *testing.T, object map[string]any, required []string) {
	t.Helper()
	for _, field := range required {
		if _, ok := object[field]; !ok {
			t.Fatalf("missing fixture field %q in %#v", field, object)
		}
	}
}

func assertNoForbiddenFields(t *testing.T, value any, forbidden []string) {
	t.Helper()
	blocked := make(map[string]struct{}, len(forbidden))
	for _, field := range forbidden {
		blocked[field] = struct{}{}
	}
	var visit func(any)
	visit = func(current any) {
		switch value := current.(type) {
		case map[string]any:
			for key, child := range value {
				if _, ok := blocked[key]; ok {
					t.Fatalf("forbidden diagnostic field %q", key)
				}
				visit(child)
			}
		case []any:
			for _, child := range value {
				visit(child)
			}
		}
	}
	visit(value)
}
