package code

import "context"

// MemoryMaintenancePhase is the lifecycle state of session-owned maintenance.
type MemoryMaintenancePhase string

const (
	MemoryMaintenanceDisabled MemoryMaintenancePhase = "disabled"
	MemoryMaintenanceRunning  MemoryMaintenancePhase = "running"
	MemoryMaintenanceDegraded MemoryMaintenancePhase = "degraded"
	MemoryMaintenanceClosing  MemoryMaintenancePhase = "closing"
	MemoryMaintenanceClosed   MemoryMaintenancePhase = "closed"
)

// MemoryMaintenanceJobHealth is a non-sensitive snapshot of one schedule.
type MemoryMaintenanceJobHealth struct {
	Name               string  `json:"name"`
	IntervalMS         uint64  `json:"intervalMs"`
	WorkerAlive        bool    `json:"workerAlive"`
	RunInProgress      bool    `json:"runInProgress"`
	SuccessfulRuns     uint64  `json:"successfulRuns"`
	FailedRuns         uint64  `json:"failedRuns"`
	TotalAffectedItems uint64  `json:"totalAffectedItems"`
	LastAffectedItems  *uint64 `json:"lastAffectedItems"`
	LastError          *string `json:"lastError"`
}

// MemoryMaintenanceHealth reports periodic pruning and host-owned
// consolidation without exposing memory content or evidence.
type MemoryMaintenanceHealth struct {
	Phase MemoryMaintenancePhase       `json:"phase"`
	Jobs  []MemoryMaintenanceJobHealth `json:"jobs"`
}

// MemoryMaintenanceHealth returns a point-in-time snapshot for this session.
func (session *Session) MemoryMaintenanceHealth(
	ctx context.Context,
) (MemoryMaintenanceHealth, error) {
	const op = "session_memory_maintenance_health"
	if err := validateSession(session, ctx, op); err != nil {
		return MemoryMaintenanceHealth{}, err
	}
	var health MemoryMaintenanceHealth
	err := session.runtime.Request(ctx, op, session.params(), &health)
	return health, err
}
