// Package code embeds A3S Code agents in Go applications.
//
// The package is pure Go. A LocalRuntime communicates with the matching
// a3s-code-go-bridge executable over a versioned, capability-checked JSONL
// protocol. Agent and Session values are safe for the concurrent observation
// patterns documented on their methods; conversation work remains
// intentionally single-flight per Session.
package code
