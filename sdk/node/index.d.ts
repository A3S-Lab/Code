/**
 * Entry point for `@a3s-lab/code` type declarations.
 *
 * This file is hand-authored. The napi-rs build writes to `generated.d.ts`,
 * which this aggregator re-exports. Cross-boundary types that aren't
 * generated from Rust (because napi-derive would camelCase fields that
 * need to mirror the wire JSON, or because they describe discriminated
 * unions napi can't express) live in `extra-types.d.ts`. Versioned event
 * protocol types are generated from the core catalog into
 * `event-protocol-v1.d.ts`.
 *
 * Edit the Rust sources or the event artifact generator, not generated files.
 */
export * from './generated'
export * from './extra-types'
export * from './event-protocol-v1'

declare module './generated' {
  interface EventStream extends AsyncIterable<AgentEvent> {
    [Symbol.asyncIterator](): AsyncIterator<AgentEvent>
  }
}
