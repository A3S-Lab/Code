package code

import (
	"context"
	"errors"
)

// SDKCapabilitiesSchemaV1 is the stable schema identifier for the product
// capability inventory returned by Core and all official SDKs.
const SDKCapabilitiesSchemaV1 = "a3s-code/sdk-capabilities/v1"

// GetSDKCapabilities performs a bridge handshake and returns both the
// transport operations and the product-level capability descriptors. It is
// useful for hosts that want feature discovery without creating an Agent.
// The temporary local bridge is closed before this function returns.
func GetSDKCapabilities(
	ctx context.Context,
	options ...LocalRuntimeOption,
) (Capabilities, error) {
	const op = "sdk_capabilities"
	if ctx == nil {
		return Capabilities{}, invalid(op, "context cannot be nil")
	}
	runtime, err := NewLocalRuntime(ctx, options...)
	if err != nil {
		return Capabilities{}, err
	}
	capabilities, requestErr := handshake(ctx, runtime)
	closeErr := runtime.Close()
	if requestErr != nil || closeErr != nil {
		return Capabilities{}, errors.Join(requestErr, closeErr)
	}
	return capabilities, nil
}

// SDKCapabilities returns the product-level capability descriptors advertised
// by a local Core bridge without constructing an Agent.
func SDKCapabilities(
	ctx context.Context,
	options ...LocalRuntimeOption,
) ([]ProductCapability, error) {
	capabilities, err := GetSDKCapabilities(ctx, options...)
	if err != nil {
		return nil, err
	}
	return cloneProductCapabilities(capabilities.ProductCapabilities), nil
}

// SDKCapabilitiesSchema returns the stable capability inventory schema ID.
func SDKCapabilitiesSchema() string {
	return SDKCapabilitiesSchemaV1
}
