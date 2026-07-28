package code

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	goruntime "runtime"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/A3S-Lab/Code/sdk/go/v6/internal/bridge"
)

const defaultShutdownTimeout = 5 * time.Second
const maxBridgeMessageBytes = 32 * 1024 * 1024

// Runtime is the concurrency-safe transport boundary used by Agent. The
// built-in LocalRuntime owns one long-lived a3s-code-go-bridge subprocess.
type Runtime interface {
	Request(ctx context.Context, operation string, params map[string]any, result any) error
	Stream(ctx context.Context, operation string, params map[string]any) (*EventStream, error)
	Close() error
}

// EventStream carries lossless event envelopes and one terminal error. Done
// yields nil after normal completion. Callers should continuously consume
// Events until it closes, then read Done.
type EventStream struct {
	Events <-chan Event
	Done   <-chan error
}

type LocalRuntimeOption interface {
	applyLocalRuntime(*localRuntimeConfig)
}

type localRuntimeOptionFunc func(*localRuntimeConfig)

func (option localRuntimeOptionFunc) applyLocalRuntime(config *localRuntimeConfig) {
	option(config)
}

type localRuntimeConfig struct {
	binaryPath      string
	arguments       []string
	environment     []string
	shutdownTimeout time.Duration
}

// WithBridgePath selects an a3s-code-go-bridge executable. When omitted, the
// runtime reads A3S_CODE_GO_BRIDGE and then searches PATH.
func WithBridgePath(path string) LocalRuntimeOption {
	return localRuntimeOptionFunc(func(config *localRuntimeConfig) {
		config.binaryPath = path
	})
}

// WithBridgeEnvironment adds or overrides bridge-process environment entries.
func WithBridgeEnvironment(environment ...string) LocalRuntimeOption {
	values := append([]string(nil), environment...)
	return localRuntimeOptionFunc(func(config *localRuntimeConfig) {
		config.environment = append(config.environment, values...)
	})
}

func withBridgeArguments(arguments ...string) LocalRuntimeOption {
	values := append([]string(nil), arguments...)
	return localRuntimeOptionFunc(func(config *localRuntimeConfig) {
		config.arguments = append(config.arguments, values...)
	})
}

// WithShutdownTimeout bounds graceful bridge shutdown before the process is
// terminated.
func WithShutdownTimeout(timeout time.Duration) LocalRuntimeOption {
	return localRuntimeOptionFunc(func(config *localRuntimeConfig) {
		config.shutdownTimeout = timeout
	})
}

type pendingCall struct {
	response chan bridge.Envelope
	failure  chan error
	stream   *eventQueue
}

// LocalRuntime multiplexes requests and stream events over one JSONL process.
type LocalRuntime struct {
	command *exec.Cmd
	stdin   io.WriteCloser
	stderr  *lockedBuffer

	writeMu sync.Mutex
	mu      sync.Mutex
	pending map[uint64]*pendingCall

	nextID          atomic.Uint64
	closed          atomic.Bool
	closeOnce       sync.Once
	closeErr        error
	processDone     chan error
	shutdownTimeout time.Duration
}

// NewLocalRuntime starts the local bridge process.
func NewLocalRuntime(ctx context.Context, options ...LocalRuntimeOption) (*LocalRuntime, error) {
	const op = "new_local_runtime"
	if ctx == nil {
		return nil, invalid(op, "context cannot be nil")
	}
	if err := ctx.Err(); err != nil {
		return nil, contextError(op, err)
	}

	config := localRuntimeConfig{shutdownTimeout: defaultShutdownTimeout}
	for _, option := range options {
		if option != nil {
			option.applyLocalRuntime(&config)
		}
	}
	if config.shutdownTimeout <= 0 {
		return nil, invalid(op, "shutdown timeout must be greater than zero")
	}
	binary, err := resolveBridgeBinary(config.binaryPath)
	if err != nil {
		return nil, sdkError(op, CodeNotInstalled, err.Error(), err)
	}

	command := exec.Command(binary, config.arguments...)
	command.Env = mergeEnvironment(os.Environ(), config.environment)
	stdin, err := command.StdinPipe()
	if err != nil {
		return nil, sdkError(op, CodeRuntime, "cannot open bridge stdin", err)
	}
	stdout, err := command.StdoutPipe()
	if err != nil {
		return nil, sdkError(op, CodeRuntime, "cannot open bridge stdout", err)
	}
	stderr := &lockedBuffer{}
	command.Stderr = stderr
	if err := command.Start(); err != nil {
		return nil, sdkError(op, CodeRuntime, "cannot start bridge process", err)
	}

	runtime := &LocalRuntime{
		command:         command,
		stdin:           stdin,
		stderr:          stderr,
		pending:         make(map[uint64]*pendingCall),
		processDone:     make(chan error, 1),
		shutdownTimeout: config.shutdownTimeout,
	}
	go runtime.readLoop(stdout)
	go func() {
		err := command.Wait()
		runtime.closed.Store(true)
		runtime.failAll(runtime.processError("bridge process exited", err))
		runtime.processDone <- err
		close(runtime.processDone)
	}()
	return runtime, nil
}

func (runtime *LocalRuntime) Request(
	ctx context.Context,
	operation string,
	params map[string]any,
	result any,
) error {
	if err := validateRuntimeCall(runtime, ctx, operation); err != nil {
		return err
	}
	id := runtime.nextID.Add(1)
	call := &pendingCall{
		response: make(chan bridge.Envelope, 1),
		failure:  make(chan error, 1),
	}
	if err := runtime.register(id, call); err != nil {
		return err
	}
	if err := runtime.write(id, operation, params); err != nil {
		runtime.remove(id, call)
		return err
	}

	select {
	case envelope := <-call.response:
		return decodeResponse(operation, envelope, result)
	case err := <-call.failure:
		return err
	case <-ctx.Done():
		runtime.remove(id, call)
		return contextError(operation, ctx.Err())
	}
}

func (runtime *LocalRuntime) Stream(
	ctx context.Context,
	operation string,
	params map[string]any,
) (*EventStream, error) {
	if err := validateRuntimeCall(runtime, ctx, operation); err != nil {
		return nil, err
	}
	id := runtime.nextID.Add(1)
	queue := newEventQueue()
	call := &pendingCall{stream: queue}
	if err := runtime.register(id, call); err != nil {
		return nil, err
	}
	if err := runtime.write(id, operation, params); err != nil {
		runtime.remove(id, call)
		queue.finish(err)
		return nil, err
	}

	go func() {
		select {
		case <-ctx.Done():
			if runtime.remove(id, call) {
				queue.finish(contextError(operation, ctx.Err()))
				runtime.cancelRemoteSession(params)
			}
		case <-queue.finished:
		}
	}()
	return &EventStream{Events: queue.output, Done: queue.done}, nil
}

func (runtime *LocalRuntime) Close() error {
	if runtime == nil {
		return nil
	}
	runtime.closeOnce.Do(func() {
		runtime.closed.Store(true)
		runtime.closeErr = runtime.stdin.Close()
		if runtime.closeErr != nil && !errors.Is(runtime.closeErr, os.ErrClosed) {
			runtime.closeErr = sdkError(
				"runtime_close",
				CodeRuntime,
				"cannot close bridge stdin",
				runtime.closeErr,
			)
		} else {
			runtime.closeErr = nil
		}

		timer := time.NewTimer(runtime.shutdownTimeout)
		defer timer.Stop()
		select {
		case err := <-runtime.processDone:
			if err != nil && runtime.closeErr == nil {
				runtime.closeErr = runtime.processError("bridge process failed during shutdown", err)
			}
		case <-timer.C:
			if runtime.command.Process != nil {
				_ = runtime.command.Process.Kill()
			}
			runtime.closeErr = sdkError(
				"runtime_close",
				CodeBridgeTimeout,
				fmt.Sprintf("bridge did not stop within %s", runtime.shutdownTimeout),
				context.DeadlineExceeded,
			)
		}
		runtime.failAll(ErrBridgeClosed)
	})
	return runtime.closeErr
}

func (runtime *LocalRuntime) register(id uint64, call *pendingCall) error {
	runtime.mu.Lock()
	defer runtime.mu.Unlock()
	if runtime.closed.Load() {
		return sdkError("bridge_request", CodeBridgeClosed, "bridge is closed", nil)
	}
	runtime.pending[id] = call
	return nil
}

func (runtime *LocalRuntime) remove(id uint64, expected *pendingCall) bool {
	runtime.mu.Lock()
	defer runtime.mu.Unlock()
	if runtime.pending[id] != expected {
		return false
	}
	delete(runtime.pending, id)
	return true
}

func (runtime *LocalRuntime) write(id uint64, operation string, params map[string]any) error {
	if params == nil {
		params = map[string]any{}
	}
	payload, err := json.Marshal(bridge.Request{
		ProtocolVersion: bridge.ProtocolVersion,
		ID:              id,
		Operation:       operation,
		Params:          params,
	})
	if err != nil {
		return sdkError(operation, CodeInvalidRequest, "cannot encode bridge request", err)
	}
	payload = append(payload, '\n')

	runtime.writeMu.Lock()
	defer runtime.writeMu.Unlock()
	if runtime.closed.Load() {
		return sdkError(operation, CodeBridgeClosed, "bridge is closed", nil)
	}
	if _, err := runtime.stdin.Write(payload); err != nil {
		failure := runtime.processError("cannot write bridge request", err)
		runtime.terminate(failure)
		return failure
	}
	return nil
}

func (runtime *LocalRuntime) readLoop(stdout io.Reader) {
	scanner := bufio.NewScanner(stdout)
	scanner.Buffer(make([]byte, 64*1024), maxBridgeMessageBytes)
	for scanner.Scan() {
		var envelope bridge.Envelope
		if err := json.Unmarshal(scanner.Bytes(), &envelope); err != nil {
			runtime.terminate(sdkError(
				"bridge_read",
				CodeProtocol,
				"bridge emitted invalid JSON",
				err,
			))
			return
		}
		if envelope.ProtocolVersion != bridge.ProtocolVersion {
			runtime.terminate(sdkError(
				"bridge_read",
				CodeProtocol,
				fmt.Sprintf(
					"bridge emitted protocol version %d; expected %d",
					envelope.ProtocolVersion,
					bridge.ProtocolVersion,
				),
				nil,
			))
			return
		}

		runtime.mu.Lock()
		call := runtime.pending[envelope.ID]
		if call != nil && envelope.Kind == "response" {
			delete(runtime.pending, envelope.ID)
		}
		runtime.mu.Unlock()
		if call == nil {
			continue
		}

		switch envelope.Kind {
		case "event":
			if call.stream == nil {
				runtime.terminate(sdkError(
					"bridge_read",
					CodeProtocol,
					"bridge emitted an event for a non-stream request",
					nil,
				))
				return
			}
			var event Event
			if err := json.Unmarshal(envelope.Event, &event); err != nil {
				runtime.terminate(sdkError(
					"bridge_read",
					CodeProtocol,
					"cannot decode bridge event",
					err,
				))
				return
			}
			if event.Version != bridge.EventProtocolVersion {
				runtime.terminate(sdkError(
					"bridge_read",
					CodeProtocol,
					fmt.Sprintf("unsupported event protocol version %d", event.Version),
					nil,
				))
				return
			}
			call.stream.push(event)
		case "response":
			if call.stream != nil {
				call.stream.finish(decodeResponse("session_stream", envelope, nil))
			} else {
				call.response <- envelope
			}
		default:
			runtime.terminate(sdkError(
				"bridge_read",
				CodeProtocol,
				fmt.Sprintf("bridge emitted unknown envelope kind %q", envelope.Kind),
				nil,
			))
			return
		}
	}
	if err := scanner.Err(); err != nil {
		runtime.terminate(runtime.processError("cannot read bridge output", err))
	}
}

func (runtime *LocalRuntime) terminate(err error) {
	runtime.closed.Store(true)
	runtime.failAll(err)
	_ = runtime.stdin.Close()
	if runtime.command.Process != nil {
		_ = runtime.command.Process.Kill()
	}
}

func (runtime *LocalRuntime) failAll(err error) {
	runtime.mu.Lock()
	pending := runtime.pending
	runtime.pending = make(map[uint64]*pendingCall)
	runtime.mu.Unlock()
	for _, call := range pending {
		if call.stream != nil {
			call.stream.finish(err)
			continue
		}
		call.failure <- err
	}
}

func (runtime *LocalRuntime) cancelRemoteSession(params map[string]any) {
	handle, ok := params["session_handle"].(string)
	if !ok || handle == "" || runtime.closed.Load() {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	var ignored struct {
		Cancelled bool `json:"cancelled"`
	}
	_ = runtime.Request(ctx, "session_cancel", map[string]any{
		"session_handle": handle,
	}, &ignored)
}

func (runtime *LocalRuntime) processError(message string, cause error) error {
	detail := strings.TrimSpace(runtime.stderr.String())
	if detail != "" {
		message += ": " + truncate(detail, 4096)
	} else if cause != nil {
		message += ": " + cause.Error()
	}
	return sdkError("bridge", CodeRuntime, message, cause)
}

func validateRuntimeCall(runtime *LocalRuntime, ctx context.Context, operation string) error {
	if runtime == nil {
		return invalid(operation, "local runtime cannot be nil")
	}
	if ctx == nil {
		return invalid(operation, "context cannot be nil")
	}
	if operation == "" {
		return invalid("bridge_request", "operation cannot be empty")
	}
	if err := ctx.Err(); err != nil {
		return contextError(operation, err)
	}
	if runtime.closed.Load() {
		return sdkError(operation, CodeBridgeClosed, "bridge is closed", nil)
	}
	return nil
}

func decodeResponse(operation string, envelope bridge.Envelope, result any) error {
	if envelope.Kind != "response" {
		return sdkError(operation, CodeProtocol, "expected a response envelope", nil)
	}
	if !envelope.OK {
		if envelope.Error == nil {
			return sdkError(operation, CodeRuntime, "bridge request failed", nil)
		}
		code := ErrorCode(envelope.Error.Code)
		if code == "" {
			code = CodeRuntime
		}
		message := envelope.Error.Message
		if message == "" {
			message = "bridge request failed"
		}
		return sdkError(operation, code, message, nil)
	}
	if result == nil {
		return nil
	}
	if len(bytes.TrimSpace(envelope.Result)) == 0 {
		return sdkError(operation, CodeProtocol, "bridge response is missing a result", nil)
	}
	if err := json.Unmarshal(envelope.Result, result); err != nil {
		return sdkError(operation, CodeProtocol, "cannot decode bridge result", err)
	}
	return nil
}

func resolveBridgeBinary(configured string) (string, error) {
	candidate := strings.TrimSpace(configured)
	if candidate == "" {
		candidate = strings.TrimSpace(os.Getenv("A3S_CODE_GO_BRIDGE"))
	}
	if candidate == "" {
		candidate = "a3s-code-go-bridge"
	}
	if filepath.IsAbs(candidate) || strings.ContainsRune(candidate, os.PathSeparator) {
		info, err := os.Stat(candidate)
		if err != nil {
			return "", fmt.Errorf("bridge binary %q is not installed: %w", candidate, err)
		}
		if info.IsDir() {
			return "", fmt.Errorf("bridge binary %q is a directory", candidate)
		}
		return candidate, nil
	}
	resolved, err := exec.LookPath(candidate)
	if err != nil {
		return "", fmt.Errorf("bridge binary %q is not installed: %w", candidate, err)
	}
	return resolved, nil
}

func mergeEnvironment(base, overrides []string) []string {
	values := make(map[string]string, len(base)+len(overrides))
	order := make([]string, 0, len(base)+len(overrides))
	for _, entry := range append(append([]string(nil), base...), overrides...) {
		key, ok := environmentKey(entry)
		if !ok {
			continue
		}
		normalized := key
		if goruntime.GOOS == "windows" {
			normalized = strings.ToUpper(key)
		}
		if _, exists := values[normalized]; !exists {
			order = append(order, normalized)
		}
		values[normalized] = entry
	}
	merged := make([]string, 0, len(order))
	for _, key := range order {
		merged = append(merged, values[key])
	}
	return merged
}

func environmentKey(entry string) (string, bool) {
	separator := strings.IndexByte(entry, '=')
	if separator < 0 {
		return "", false
	}
	if separator == 0 {
		next := strings.IndexByte(entry[1:], '=')
		if next < 0 {
			return "", false
		}
		separator = next + 1
	}
	return entry[:separator], true
}

func truncate(value string, limit int) string {
	if len(value) <= limit {
		return value
	}
	return value[:limit] + "..."
}

type lockedBuffer struct {
	mu     sync.Mutex
	buffer bytes.Buffer
}

func (buffer *lockedBuffer) Write(data []byte) (int, error) {
	buffer.mu.Lock()
	defer buffer.mu.Unlock()
	return buffer.buffer.Write(data)
}

func (buffer *lockedBuffer) String() string {
	buffer.mu.Lock()
	defer buffer.mu.Unlock()
	return buffer.buffer.String()
}

type eventQueue struct {
	mu       sync.Mutex
	events   []Event
	closed   bool
	err      error
	wake     chan struct{}
	output   chan Event
	done     chan error
	finished chan struct{}
	once     sync.Once
}

func newEventQueue() *eventQueue {
	queue := &eventQueue{
		wake:     make(chan struct{}, 1),
		output:   make(chan Event),
		done:     make(chan error, 1),
		finished: make(chan struct{}),
	}
	go queue.run()
	return queue
}

func (queue *eventQueue) push(event Event) {
	queue.mu.Lock()
	if !queue.closed {
		queue.events = append(queue.events, event)
	}
	queue.mu.Unlock()
	queue.notify()
}

func (queue *eventQueue) finish(err error) {
	queue.once.Do(func() {
		queue.mu.Lock()
		queue.closed = true
		queue.err = err
		queue.mu.Unlock()
		queue.notify()
	})
}

func (queue *eventQueue) notify() {
	select {
	case queue.wake <- struct{}{}:
	default:
	}
}

func (queue *eventQueue) run() {
	defer close(queue.output)
	defer close(queue.done)
	defer close(queue.finished)
	for {
		queue.mu.Lock()
		if len(queue.events) > 0 {
			event := queue.events[0]
			queue.events[0] = Event{}
			queue.events = queue.events[1:]
			queue.mu.Unlock()
			queue.output <- event
			continue
		}
		closed := queue.closed
		err := queue.err
		queue.mu.Unlock()
		if closed {
			queue.done <- err
			return
		}
		<-queue.wake
	}
}
