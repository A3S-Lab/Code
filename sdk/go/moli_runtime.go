package code

import "context"

// DefaultMoliVersion is the pinned Moli release shipped by the Code 8.1
// runtime manifest. Keep this value in sync with
// core/src/moli_runtime/manifest.rs; the bridge remains authoritative for
// runtime validation and provisioning.
const DefaultMoliVersion = "1.1.1"

// GetMoliRuntimeInfo returns secret-free diagnostics for the Moli resolution
// path. It does not download or mutate the shared cache. The package-level
// helper starts and closes a short-lived local bridge; use Agent.MoliRuntimeInfo
// when an Agent is already available.
func GetMoliRuntimeInfo(
	ctx context.Context,
	config *HeadlessConfig,
	options ...LocalRuntimeOption,
) (MoliRuntimeStatus, error) {
	const op = "moli_runtime_info"
	if ctx == nil {
		return MoliRuntimeStatus{}, invalid(op, "context cannot be nil")
	}
	runtime, err := NewLocalRuntime(ctx, options...)
	if err != nil {
		return MoliRuntimeStatus{}, err
	}
	defer runtime.Close()
	return moliRuntimeInfoWithRuntime(ctx, runtime, config)
}

// MoliRuntimeInfo is a concise alias for GetMoliRuntimeInfo.
func MoliRuntimeInfo(
	ctx context.Context,
	config *HeadlessConfig,
	options ...LocalRuntimeOption,
) (MoliRuntimeStatus, error) {
	return GetMoliRuntimeInfo(ctx, config, options...)
}

// EnsureMoli verifies a packaged/shared-cache runtime or downloads the pinned
// release into the cross-process shared cache, returning its executable path.
func EnsureMoli(
	ctx context.Context,
	config *HeadlessConfig,
	options ...LocalRuntimeOption,
) (string, error) {
	const op = "moli_ensure"
	if ctx == nil {
		return "", invalid(op, "context cannot be nil")
	}
	runtime, err := NewLocalRuntime(ctx, options...)
	if err != nil {
		return "", err
	}
	defer runtime.Close()
	return ensureMoliWithRuntime(ctx, runtime, config)
}

// MoliDefaultVersion returns the pinned runtime version bundled by this Code
// release.
func MoliDefaultVersion() string {
	// The bridge is the source of truth for the Rust manifest. Keep this value
	// in the Go API only as a discoverability fallback; callers that need the
	// full status should use GetMoliRuntimeInfo.
	return DefaultMoliVersion
}

// MoliRuntimeInfo queries the same operation through an existing runtime.
func (agent *Agent) MoliRuntimeInfo(ctx context.Context, config *HeadlessConfig) (MoliRuntimeStatus, error) {
	const op = "moli_runtime_info"
	if err := validateAgent(agent, ctx, op); err != nil {
		return MoliRuntimeStatus{}, err
	}
	return moliRuntimeInfoWithRuntime(ctx, agent.runtime, config)
}

// EnsureMoli provisions Moli through the runtime owned by this Agent.
func (agent *Agent) EnsureMoli(ctx context.Context, config *HeadlessConfig) (string, error) {
	const op = "moli_ensure"
	if err := validateAgent(agent, ctx, op); err != nil {
		return "", err
	}
	return ensureMoliWithRuntime(ctx, agent.runtime, config)
}

func moliRuntimeInfoWithRuntime(
	ctx context.Context,
	runtime Runtime,
	config *HeadlessConfig,
) (MoliRuntimeStatus, error) {
	var result MoliRuntimeStatus
	params := map[string]any{}
	if config != nil {
		params["config"] = config
	}
	if err := runtime.Request(ctx, "moli_runtime_info", params, &result); err != nil {
		return MoliRuntimeStatus{}, err
	}
	return result, nil
}

func ensureMoliWithRuntime(
	ctx context.Context,
	runtime Runtime,
	config *HeadlessConfig,
) (string, error) {
	params := map[string]any{}
	if config != nil {
		params["config"] = config
	}
	var result struct {
		Path string `json:"path"`
	}
	if err := runtime.Request(ctx, "moli_ensure", params, &result); err != nil {
		return "", err
	}
	if result.Path == "" {
		return "", sdkError("moli_ensure", CodeProtocol, "bridge returned an empty Moli path", nil)
	}
	return result.Path, nil
}
