#![cfg(test)]

use super::*;

struct MutatingWriter {
    output: File,
    source: std::path::PathBuf,
    changed: bool,
}
impl Write for MutatingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.output.write(bytes)?;
        if !self.changed {
            self.changed = true;
            std::fs::write(&self.source, vec![b'z'; 65536])?;
        }
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

#[test]
fn actual_source_rewrite_during_streaming_copy_cannot_publish_success() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source");
    std::fs::write(&path, vec![b'a'; 65536]).unwrap();
    let mut source = File::open(&path).unwrap();
    let mut output = MutatingWriter {
        output: File::create(root.path().join("partial")).unwrap(),
        source: path,
        changed: false,
    };
    let error = bounded_copy(&mut source, &mut output, 65536).unwrap_err();
    assert!(output.changed);
    assert_eq!(error.reason, AcceptanceReasonV1::FreezeRace);
}

#[test]
fn non_utf8_directory_name_decoder_refuses_without_lossy_conversion() {
    assert_eq!(
        exact_entry_name(&[0xff]).unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
    assert_eq!(
        exact_entry_name(" 金额\\exact ".as_bytes()).unwrap(),
        " 金额\\exact "
    );
}
