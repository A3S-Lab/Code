package code

import (
	"context"
	"sync"
)

type PipelineContext struct {
	Previous *StepOutcome
	Item     any
}

// PipelineStage returns the next agent step for an item. Returning nil stops
// that item's chain.
type PipelineStage func(context.Context, PipelineContext) (*AgentStepSpec, error)

// Pipeline runs every item through the supplied stages without a barrier
// between stages. Output order matches input order.
func (session *Session) Pipeline(
	ctx context.Context,
	items []any,
	stages []PipelineStage,
) ([]*StepOutcome, error) {
	const op = "session_pipeline"
	if err := validateSession(session, ctx, op); err != nil {
		return nil, err
	}
	if len(stages) == 0 {
		return nil, invalid(op, "at least one stage is required")
	}

	outcomes := make([]*StepOutcome, len(items))
	errorsByItem := make([]error, len(items))
	var wait sync.WaitGroup
	wait.Add(len(items))

	for index, item := range items {
		index, item := index, item
		go func() {
			defer wait.Done()
			var previous *StepOutcome
			for _, stage := range stages {
				if stage == nil {
					continue
				}
				spec, err := stage(ctx, PipelineContext{
					Previous: previous,
					Item:     item,
				})
				if err != nil {
					errorsByItem[index] = err
					return
				}
				if spec == nil {
					break
				}
				previous, err = session.WorkflowStep(ctx, *spec)
				if err != nil {
					errorsByItem[index] = err
					return
				}
				if !previous.Success {
					break
				}
			}
			outcomes[index] = previous
		}()
	}
	wait.Wait()

	for _, err := range errorsByItem {
		if err != nil {
			return outcomes, err
		}
	}
	return outcomes, nil
}
