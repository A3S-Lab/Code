package code

import "testing"

func TestSDKCapabilitiesSchemaIsStable(t *testing.T) {
	if got := SDKCapabilitiesSchema(); got != SDKCapabilitiesSchemaV2 {
		t.Fatalf("SDKCapabilitiesSchema = %q, want %q", got, SDKCapabilitiesSchemaV2)
	}
}

func TestDefaultMoliVersionIsPinned(t *testing.T) {
	if got := MoliDefaultVersion(); got != DefaultMoliVersion {
		t.Fatalf("MoliDefaultVersion = %q, want %q", got, DefaultMoliVersion)
	}
}
