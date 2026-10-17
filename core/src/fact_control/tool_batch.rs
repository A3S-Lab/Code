//! Read-only batch admission: the built-in read allowlist, plus registered
//! tools whose own capabilities declare a parallel-safe call.

use super::is_parallel_safe_read;
use crate::tools::ToolExecutor;
use a3s_effect::coding::ToolCall;

/// `call` may run concurrently with `batch_len - 1` other admitted reads.
pub(super) fn parallel_read(executor: &ToolExecutor, call: &ToolCall, batch_len: usize) -> bool {
    is_parallel_safe_read(&call.name)
        || executor
            .registry()
            .capabilities(&call.name, &call.args)
            .is_some_and(|capabilities| {
                capabilities.allows_parallel_batch() && batch_len <= capabilities.max_parallelism
            })
}

pub(super) fn all_parallel_reads(executor: &ToolExecutor, calls: &[ToolCall]) -> bool {
    calls.len() > 1
        && calls
            .iter()
            .all(|call| parallel_read(executor, call, calls.len()))
}
