use futures::FutureExt;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use tokio::task::JoinSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OrderedParallelError {
    Panicked(String),
    JoinFailed(String),
}

impl fmt::Display for OrderedParallelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Panicked(message) => write!(f, "parallel branch panicked: {message}"),
            Self::JoinFailed(message) => write!(f, "parallel branch join failed: {message}"),
        }
    }
}

impl std::error::Error for OrderedParallelError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrderedParallelResult<T> {
    pub index: usize,
    pub output: Result<T, OrderedParallelError>,
}

#[cfg(test)]
pub(crate) async fn run_ordered_parallel<I, F, Fut, T>(
    items: Vec<I>,
    run: F,
) -> Vec<OrderedParallelResult<T>>
where
    I: Send + 'static,
    F: Fn(usize, I) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let item_count = items.len();
    run_ordered_parallel_with_limit(items, item_count.max(1), run).await
}

pub(crate) async fn run_ordered_parallel_with_limit<I, F, Fut, T>(
    items: Vec<I>,
    max_concurrency: usize,
    run: F,
) -> Vec<OrderedParallelResult<T>>
where
    I: Send + 'static,
    F: Fn(usize, I) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let mut join_set = JoinSet::new();
    let mut active_indexes = HashMap::new();
    let mut pending = items.into_iter().enumerate();
    let max_concurrency = max_concurrency.max(1);
    let mut active = 0usize;

    while active < max_concurrency {
        let Some((index, item)) = pending.next() else {
            break;
        };
        let run = run.clone();
        let handle = join_set.spawn(async move {
            let output = AssertUnwindSafe(run(index, item))
                .catch_unwind()
                .await
                .map_err(|payload| {
                    OrderedParallelError::Panicked(panic_payload_to_string(payload))
                });
            OrderedParallelResult { index, output }
        });
        active_indexes.insert(handle.id(), index);
        active += 1;
    }

    let mut results = Vec::new();
    while active > 0 {
        if let Some(result) = join_set.join_next_with_id().await {
            active -= 1;
            match result {
                Ok((task_id, mut result)) => {
                    let tracked_index =
                        take_ordered_index(&mut active_indexes, task_id).unwrap_or(result.index);
                    if tracked_index != result.index {
                        tracing::error!(
                            tracked_index,
                            reported_index = result.index,
                            "ordered parallel branch returned a mismatched task index"
                        );
                        result.index = tracked_index;
                    }
                    results.push(result);
                }
                Err(error) => {
                    let index = take_ordered_index(&mut active_indexes, error.id()).unwrap_or_else(|| {
                        tracing::error!(%error, "ordered parallel join failed without a tracked index");
                        usize::MAX
                    });
                    results.push(OrderedParallelResult {
                        index,
                        output: Err(OrderedParallelError::JoinFailed(error.to_string())),
                    });
                }
            }
        }

        while active < max_concurrency {
            let Some((index, item)) = pending.next() else {
                break;
            };
            let run = run.clone();
            let handle = join_set.spawn(async move {
                let output = AssertUnwindSafe(run(index, item))
                    .catch_unwind()
                    .await
                    .map_err(|payload| {
                        OrderedParallelError::Panicked(panic_payload_to_string(payload))
                    });
                OrderedParallelResult { index, output }
            });
            active_indexes.insert(handle.id(), index);
            active += 1;
        }
    }

    results.sort_by_key(|result| result.index);
    results
}

fn take_ordered_index(
    active_indexes: &mut HashMap<tokio::task::Id, usize>,
    task_id: tokio::task::Id,
) -> Option<usize> {
    active_indexes.remove(&task_id)
}

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Barrier;

    #[tokio::test]
    async fn preserves_input_order_when_completion_order_differs() {
        let barrier = std::sync::Arc::new(Barrier::new(2));
        let results = run_ordered_parallel(vec![120_u64, 10], {
            let barrier = std::sync::Arc::clone(&barrier);
            move |_index, delay_ms| {
                let barrier = std::sync::Arc::clone(&barrier);
                async move {
                    barrier.wait().await;
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    delay_ms
                }
            }
        })
        .await;

        let values = results
            .into_iter()
            .map(|result| result.output.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(values, vec![120, 10]);
    }

    #[tokio::test]
    async fn catches_branch_panics_without_dropping_other_results() {
        let results = run_ordered_parallel(vec![0_u8, 1], |_index, value| async move {
            assert!(value != 0, "boom");
            value
        })
        .await;

        assert_eq!(results.len(), 2);
        assert!(matches!(
            results[0].output,
            Err(OrderedParallelError::Panicked(_))
        ));
        assert_eq!(results[1].output, Ok(1));
    }

    #[tokio::test]
    async fn respects_concurrency_limit() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let results = run_ordered_parallel_with_limit(vec![1_u8, 2, 3, 4], 2, {
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            move |_index, value| {
                let active = Arc::clone(&active);
                let max_active = Arc::clone(&max_active);
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    value
                }
            }
        })
        .await;

        assert_eq!(max_active.load(Ordering::SeqCst), 2);
        assert_eq!(
            results
                .into_iter()
                .map(|result| result.output.unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[tokio::test]
    async fn aborted_join_retains_its_input_index() {
        let mut join_set = JoinSet::new();
        let handle = join_set.spawn(async { std::future::pending::<u8>().await });
        let mut active_indexes = HashMap::from([(handle.id(), 3)]);
        handle.abort();

        let error = join_set
            .join_next_with_id()
            .await
            .expect("aborted task should settle")
            .expect_err("aborted task should return JoinError");

        assert_eq!(take_ordered_index(&mut active_indexes, error.id()), Some(3));
        assert!(active_indexes.is_empty());
    }
}
