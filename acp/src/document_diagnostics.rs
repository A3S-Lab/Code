use super::*;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Publication {
    publisher_id: String,
    sequence: u64,
    version: u64,
    items: Vec<Diagnostic>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Diagnostic {
    start: Position,
    end: Position,
    severity: Severity,
    message: String,
}

#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
#[serde(deny_unknown_fields)]
struct Position {
    line: u32,
    character: u32,
}

enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}
impl Severity {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "error" => Ok(Self::Error),
            "warning" => Ok(Self::Warning),
            "information" => Ok(Self::Information),
            "hint" => Ok(Self::Hint),
            _ => anyhow::bail!("invalid diagnostic severity"),
        }
    }
    fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "information",
            Self::Hint => "hint",
        }
    }
}
impl Serialize for Severity {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for Severity {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

pub(super) fn validate(publications: &[Publication], snapshot: &Snapshot) -> Result<()> {
    anyhow::ensure!(
        publications.len() <= 16 && serde_json::to_vec(publications)?.len() <= 96 * 1024,
        "diagnostic observation exceeds limit"
    );
    let mut publishers = HashSet::new();
    for publication in publications {
        let id = uuid::Uuid::parse_str(&publication.publisher_id)?;
        anyhow::ensure!(
            id.to_string() == publication.publisher_id
                && publishers.insert(id)
                && publication.sequence > 0
                && publication.sequence <= 9_007_199_254_740_991
                && publication.version == snapshot.version
                && publication.items.len() <= 128
                && serde_json::to_vec(&publication.items)?.len() <= 16 * 1024,
            "invalid diagnostic publication binding"
        );
        for item in &publication.items {
            anyhow::ensure!(
                !item.message.is_empty()
                    && item.message.len() <= 2048
                    && !item.message.contains('\0')
                    && item.start <= item.end
                    && position_valid(&snapshot.text, item.start)
                    && position_valid(&snapshot.text, item.end),
                "invalid diagnostic range or message"
            );
        }
    }
    Ok(())
}

fn position_valid(text: &str, position: Position) -> bool {
    let Some(line) = text.split('\n').nth(position.line as usize) else {
        return false;
    };
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut offset = 0;
    for character in line.chars() {
        if offset == position.character as usize {
            return true;
        }
        offset += character.len_utf16();
    }
    offset == position.character as usize
}
