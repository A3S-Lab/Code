//! Process output reading utility

use super::types::{ToolEventSender, ToolStreamEvent};
use super::MAX_OUTPUT_SIZE;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Child;

pub(crate) async fn read_process_output(
    child: &mut Child,
    timeout_secs: u64,
    event_tx: Option<&ToolEventSender>,
) -> (String, bool) {
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut output = String::new();
    let mut total_size = 0usize;

    let timeout = tokio::time::Duration::from_secs(timeout_secs);
    let result = tokio::time::timeout(timeout, async {
        loop {
            tokio::select! {
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            if total_size < MAX_OUTPUT_SIZE {
                                output.push_str(&line);
                                output.push('\n');
                                total_size += line.len() + 1;
                            }
                            if let Some(tx) = event_tx {
                                let mut delta = line;
                                delta.push('\n');
                                tx.send(ToolStreamEvent::OutputDelta(delta)).await.ok();
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            if total_size < MAX_OUTPUT_SIZE {
                                output.push_str(&line);
                                output.push('\n');
                                total_size += line.len() + 1;
                            }
                            if let Some(tx) = event_tx {
                                let mut delta = line;
                                delta.push('\n');
                                tx.send(ToolStreamEvent::OutputDelta(delta)).await.ok();
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                }
            }
        }
    })
    .await;

    if result.is_err() {
        child.kill().await.ok();
        return (output, true);
    }

    (output, false)
}
