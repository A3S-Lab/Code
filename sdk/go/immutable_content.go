package code

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

const (
	defaultImmutableContentTimeout = 30 * time.Second
	maxImmutableContentTimeout     = 5 * time.Minute
)

// ImmutableContentAdapterOptions injects a host-owned create-only retention
// adapter through SessionOptions (SDK-IMM1). Code owns digests and binding;
// the host owns authorization and object lifecycle.
type ImmutableContentAdapterOptions struct {
	AuthorityDigest string
	MaximumBytes    uint64
	AdapterName     string
	Put             func(ctx context.Context, req SdkImmutableContentWriteRequestV1) (*ImmutableContentReferenceV1, error)
	Timeout         time.Duration
}

// SdkImmutableContentWriteRequestV1 is the cross-language wire form for one
// immutable-content put. Binding and descriptor retain Core snake_case JSON.
type SdkImmutableContentWriteRequestV1 struct {
	Binding       json.RawMessage `json:"binding"`
	Descriptor    json.RawMessage `json:"descriptor"`
	ContentBase64 string          `json:"contentBase64"`
}

// ImmutableContentReferenceV1 is the provider-neutral reference returned by Put.
type ImmutableContentReferenceV1 struct {
	Schema          string `json:"schema"`
	BindingDigest   string `json:"binding_digest"`
	URI             string `json:"uri"`
	ContentDigest   string `json:"content_digest"`
	MediaType       string `json:"media_type"`
	SizeBytes       uint64 `json:"size_bytes"`
	ReferenceDigest string `json:"reference_digest"`
}

type immutableContentAdapterWireOptions struct {
	HandlerID       string `json:"handler_id"`
	AuthorityDigest string `json:"authority_digest"`
	MaximumBytes    uint64 `json:"maximum_bytes"`
	AdapterName     string `json:"adapter_name"`
	TimeoutMS       uint64 `json:"timeout_ms"`
}

func prepareImmutableContentAdapterOptions(
	runtime Runtime,
	prepared any,
	options *SessionOptions,
) (any, string, error) {
	if options == nil || options.ImmutableContentAdapter == nil {
		return prepared, "", nil
	}
	adapter := options.ImmutableContentAdapter
	if adapter.Put == nil {
		return nil, "", invalid("immutable_content_adapter", "Put callback cannot be nil")
	}
	authorityDigest := strings.TrimSpace(adapter.AuthorityDigest)
	if authorityDigest == "" {
		return nil, "", invalid("immutable_content_adapter", "authority_digest is required")
	}
	if adapter.MaximumBytes == 0 {
		return nil, "", invalid("immutable_content_adapter", "maximum_bytes must be positive")
	}
	adapterName := strings.TrimSpace(adapter.AdapterName)
	if adapterName == "" {
		return nil, "", invalid("immutable_content_adapter", "adapter_name is required")
	}
	timeout := adapter.Timeout
	if timeout == 0 {
		timeout = defaultImmutableContentTimeout
	}
	if timeout < time.Millisecond || timeout > maxImmutableContentTimeout {
		return nil, "", invalid(
			"immutable_content_adapter",
			"timeout must be positive and no more than five minutes",
		)
	}
	callbacks, ok := runtime.(callbackRuntime)
	if !ok {
		return nil, "", sdkError(
			"immutable_content_adapter",
			CodeUnavailable,
			"runtime does not support Go callbacks",
			nil,
		)
	}
	handlerID, err := callbacks.registerCallback(
		func(ctx context.Context, method string, payload json.RawMessage) (any, error) {
			if method != "put" {
				return nil, fmt.Errorf("unexpected immutable content callback method %q", method)
			}
			var request SdkImmutableContentWriteRequestV1
			if err := json.Unmarshal(payload, &request); err != nil {
				return nil, err
			}
			reference, putErr := adapter.Put(ctx, request)
			if putErr != nil {
				return nil, putErr
			}
			if reference == nil {
				return nil, fmt.Errorf("immutable content Put returned nil reference")
			}
			return reference, nil
		},
	)
	if err != nil {
		return nil, "", err
	}

	wire, err := sessionOptionsWireMap(prepared, options)
	if err != nil {
		callbacks.unregisterCallback(handlerID)
		return nil, "", err
	}
	wire["immutable_content_adapter"] = immutableContentAdapterWireOptions{
		HandlerID:       handlerID,
		AuthorityDigest: authorityDigest,
		MaximumBytes:    adapter.MaximumBytes,
		AdapterName:     adapterName,
		TimeoutMS:       uint64(timeout / time.Millisecond),
	}

	return wire, handlerID, nil
}

func sessionOptionsWireMap(prepared any, options *SessionOptions) (map[string]any, error) {
	if wire, ok := prepared.(map[string]any); ok {
		return wire, nil
	}
	source := prepared
	if source == nil {
		source = options
	}
	encoded, err := json.Marshal(source)
	if err != nil {
		return nil, sdkError(
			"immutable_content_adapter",
			CodeSerialization,
			"cannot encode session options",
			err,
		)
	}
	var wire map[string]any
	if err := json.Unmarshal(encoded, &wire); err != nil {
		return nil, sdkError(
			"immutable_content_adapter",
			CodeSerialization,
			"cannot prepare session options",
			err,
		)
	}
	return wire, nil
}
