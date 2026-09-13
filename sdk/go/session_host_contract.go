package code

import (
	"context"
	"encoding/json"
	"strings"
)

// RepublishInheritedMCPTools rebuilds executor tool registrations from
// inherited MCP managers. Call after Agent.SyncGlobalMCPServers so a live
// session picks up global connector changes. Session-local servers stay.
func (session *Session) RepublishInheritedMCPTools(ctx context.Context) error {
	const op = "session_republish_inherited_mcp_tools"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	return session.runtime.Request(ctx, op, session.params(), nil)
}

// InheritsMCPManagers reports whether this session inherits a shared MCP manager.
func (session *Session) InheritsMCPManagers(ctx context.Context) (bool, error) {
	const op = "session_inherits_mcp_managers"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	var result struct {
		Inherits bool `json:"inherits"`
	}
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return false, err
	}
	return result.Inherits, nil
}

// SessionReviewStore returns the durable session-review store JSON.
func (session *Session) SessionReviewStore(ctx context.Context) (json.RawMessage, error) {
	const op = "session_review_store"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	return session.requestRaw(ctx, op, session.params())
}

// ReviewScenarioIDs returns registered review scenario ids, sorted.
func (session *Session) ReviewScenarioIDs(ctx context.Context) ([]string, error) {
	const op = "session_review_scenario_ids"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	var result struct {
		IDs []string `json:"ids"`
	}
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return nil, err
	}
	if result.IDs == nil {
		return []string{}, nil
	}
	return result.IDs, nil
}

// PendingSessionReviewFindings returns pending findings that inject into the next main turn.
func (session *Session) PendingSessionReviewFindings(ctx context.Context) (json.RawMessage, error) {
	const op = "session_pending_review_findings"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	return session.requestRaw(ctx, op, session.params())
}

// PendingSessionReviewFindingsForScenario returns pending findings for one scenario.
func (session *Session) PendingSessionReviewFindingsForScenario(
	ctx context.Context,
	scenarioID string,
) (json.RawMessage, error) {
	const op = "session_pending_review_findings_for_scenario"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if strings.TrimSpace(scenarioID) == "" {
		return nil, invalid(op, "scenario id cannot be empty")
	}
	params := session.params()
	params["scenario_id"] = scenarioID
	return session.requestRaw(ctx, op, params)
}

// AddressedSessionReviewFindings returns findings waiting for reviewer acceptance.
func (session *Session) AddressedSessionReviewFindings(ctx context.Context) (json.RawMessage, error) {
	const op = "session_addressed_review_findings"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	return session.requestRaw(ctx, op, session.params())
}

// UpsertSessionReviewFinding inserts or replaces a finding. New findings must be pending.
func (session *Session) UpsertSessionReviewFinding(ctx context.Context, finding json.RawMessage) error {
	const op = "session_upsert_review_finding"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if len(finding) == 0 || !json.Valid(finding) {
		return invalid(op, "finding must be a JSON object")
	}
	params := session.params()
	params["finding"] = json.RawMessage(finding)
	return session.runtime.Request(ctx, op, params, nil)
}

// MarkSessionReviewAddressed marks a pending finding addressed by a main-agent run.
func (session *Session) MarkSessionReviewAddressed(
	ctx context.Context,
	findingID string,
	runID string,
	atMS uint64,
) error {
	const op = "session_mark_review_addressed"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(findingID) == "" || strings.TrimSpace(runID) == "" {
		return invalid(op, "finding id and run id cannot be empty")
	}
	params := session.params()
	params["finding_id"] = findingID
	params["run_id"] = runID
	params["at_ms"] = atMS
	return session.runtime.Request(ctx, op, params, nil)
}

// AcceptSessionReviewFinding accepts an addressed finding.
func (session *Session) AcceptSessionReviewFinding(
	ctx context.Context,
	findingID string,
	reviewID string,
	atMS uint64,
) error {
	const op = "session_accept_review_finding"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(findingID) == "" || strings.TrimSpace(reviewID) == "" {
		return invalid(op, "finding id and review id cannot be empty")
	}
	params := session.params()
	params["finding_id"] = findingID
	params["review_id"] = reviewID
	params["at_ms"] = atMS
	return session.runtime.Request(ctx, op, params, nil)
}

// ReopenSessionReviewFinding reopens an addressed finding.
func (session *Session) ReopenSessionReviewFinding(
	ctx context.Context,
	findingID string,
	reason string,
	atMS uint64,
) error {
	const op = "session_reopen_review_finding"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(findingID) == "" {
		return invalid(op, "finding id cannot be empty")
	}
	params := session.params()
	params["finding_id"] = findingID
	params["reason"] = reason
	params["at_ms"] = atMS
	return session.runtime.Request(ctx, op, params, nil)
}

// WaiveSessionReviewFinding waives a pending finding.
func (session *Session) WaiveSessionReviewFinding(ctx context.Context, findingID string, atMS uint64) error {
	const op = "session_waive_review_finding"
	if err := validateSession(session, ctx, op); err != nil {
		return err
	}
	if strings.TrimSpace(findingID) == "" {
		return invalid(op, "finding id cannot be empty")
	}
	params := session.params()
	params["finding_id"] = findingID
	params["at_ms"] = atMS
	return session.runtime.Request(ctx, op, params, nil)
}

// ConcealLatestFindingsAddressTurn demotes a mistaken product-visible address
// user prompt to wire-only and returns how many messages changed.
func (session *Session) ConcealLatestFindingsAddressTurn(ctx context.Context) (uint, error) {
	const op = "session_conceal_findings_address_turn"
	if err := validateSession(session, ctx, op); err != nil {
		return 0, err
	}
	var result struct {
		Concealed uint `json:"concealed"`
	}
	if err := session.runtime.Request(ctx, op, session.params(), &result); err != nil {
		return 0, err
	}
	return result.Concealed, nil
}

// RecordOutcome records accept, revert, or reject against a change-set digest.
// A secret-shaped constraint is not stored. Returns whether the ledger kept it.
func (session *Session) RecordOutcome(
	ctx context.Context,
	outcome string,
	changeDigest string,
	constraint string,
) (bool, error) {
	const op = "session_record_outcome"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	switch outcome {
	case "accept", "revert", "reject":
	default:
		return false, invalid(op, "outcome must be accept, revert, or reject")
	}
	params := session.params()
	params["outcome"] = outcome
	params["change_digest"] = changeDigest
	params["constraint"] = constraint
	var result struct {
		Stored bool `json:"stored"`
	}
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return false, err
	}
	return result.Stored, nil
}

// NotePromotedDigest remembers a promoted isolation digest without activating recall.
func (session *Session) NotePromotedDigest(ctx context.Context, digest string) (bool, error) {
	const op = "session_note_promoted_digest"
	if err := validateSession(session, ctx, op); err != nil {
		return false, err
	}
	params := session.params()
	params["digest"] = digest
	var result struct {
		Stored bool `json:"stored"`
	}
	if err := session.runtime.Request(ctx, op, params, &result); err != nil {
		return false, err
	}
	return result.Stored, nil
}

// OutcomeLedgerSnapshot returns the ledger the next turn will serve.
func (session *Session) OutcomeLedgerSnapshot(ctx context.Context) (json.RawMessage, error) {
	const op = "session_outcome_ledger_snapshot"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	return session.requestRaw(ctx, op, session.params())
}

func (session *Session) requestRaw(ctx context.Context, op string, params map[string]any) (json.RawMessage, error) {
	var raw json.RawMessage
	if err := session.runtime.Request(ctx, op, params, &raw); err != nil {
		return nil, err
	}
	return raw, nil
}
