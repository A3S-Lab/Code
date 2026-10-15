package code

import (
	"context"
	"encoding/json"
	"testing"
)

type panickingJSON struct{}

func (panickingJSON) MarshalJSON() ([]byte, error) { panic("host-secret") }

type panickingError struct{}

func (panickingError) Error() string { panic("host-secret") }

type failingJSON struct{}

func (failingJSON) MarshalJSON() ([]byte, error) { return nil, panickingError{} }

func TestHostCallbackPanicRefusal(t *testing.T) {
	for _, name := range []string{"handler", "marshaler", "error", "marshal-error"} {
		t.Run(name, func(t *testing.T) {
			result, err := invokeHostCallback(context.Background(), "fixture", json.RawMessage(`{}`), func(context.Context, string, json.RawMessage) (any, error) {
				if name == "handler" {
					panic("host-secret")
				}
				if name == "error" {
					return nil, panickingError{}
				}
				if name == "marshal-error" {
					return failingJSON{}, nil
				}
				return panickingJSON{}, nil
			})
			if result != nil || err == nil || err.Error() != "host callback panicked" {
				t.Fatalf("panic was not safely refused: %s %v", result, err)
			}
		})
	}
}
