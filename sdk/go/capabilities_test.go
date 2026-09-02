package code

import "testing"

func TestSDKCapabilitiesSchemaIsStable(t *testing.T) {
	if got := SDKCapabilitiesSchema(); got != SDKCapabilitiesSchemaV1 {
		t.Fatalf("SDKCapabilitiesSchema = %q, want %q", got, SDKCapabilitiesSchemaV1)
	}
}

func TestDefaultMoliVersionIsPinned(t *testing.T) {
	if got := MoliDefaultVersion(); got != DefaultMoliVersion {
		t.Fatalf("MoliDefaultVersion = %q, want %q", got, DefaultMoliVersion)
	}
}
