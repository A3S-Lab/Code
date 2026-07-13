use serde_json::Value;

pub(crate) const DEFAULT_PAGE_LIMIT: usize = 200;
pub(crate) const MAX_PAGE_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageRequest {
    pub(crate) offset: usize,
    pub(crate) requested_limit: usize,
    pub(crate) limit: usize,
}

impl PageRequest {
    pub(crate) fn parse(
        args: &Value,
        default_limit: usize,
        max_limit: usize,
    ) -> Result<Self, String> {
        let requested_limit = match args.get("limit") {
            Some(value) => value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| "limit must be a positive integer".to_string())?,
            None => default_limit,
        };
        let offset = match args.get("cursor") {
            // Some structured-output providers materialize omitted optional
            // strings as "". Treat that neutral value exactly like omission;
            // a first-page read must not fail merely because the provider made
            // an optional default explicit.
            Some(Value::String(value)) if value.trim().is_empty() => 0,
            Some(Value::String(value)) => value
                .parse::<usize>()
                .map_err(|_| "cursor is invalid or expired".to_string())?,
            Some(Value::Number(value)) => value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| "cursor must be a non-negative integer string".to_string())?,
            Some(_) => return Err("cursor must be a non-negative integer string".to_string()),
            None => 0,
        };
        Ok(Self {
            offset,
            requested_limit,
            limit: requested_limit.min(max_limit),
        })
    }

    pub(crate) fn page<T>(self, items: Vec<T>) -> Result<Page<T>, String> {
        let total_items = items.len();
        if self.offset > total_items {
            return Err(format!(
                "cursor offset {} exceeds result length {}",
                self.offset, total_items
            ));
        }
        let start = self.offset;
        let end = start.saturating_add(self.limit).min(total_items);
        let items = items
            .into_iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect();
        Ok(Page {
            items,
            total_items,
            offset: start,
            requested_limit: self.requested_limit,
            applied_limit: self.limit,
            next_cursor: (end < total_items).then(|| end.to_string()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Page<T> {
    pub(crate) items: Vec<T>,
    pub(crate) total_items: usize,
    pub(crate) offset: usize,
    pub(crate) requested_limit: usize,
    pub(crate) applied_limit: usize,
    pub(crate) next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub(crate) fn metadata(&self) -> Value {
        serde_json::json!({
            "offset": self.offset,
            "requested_limit": self.requested_limit,
            "applied_limit": self.applied_limit,
            "returned_items": self.items.len(),
            "total_items": self.total_items,
            "next_cursor": self.next_cursor,
            "truncated": self.next_cursor.is_some(),
            "limit_clamped": self.requested_limit != self.applied_limit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_request_clamps_limit_and_returns_resume_cursor() {
        let request = PageRequest::parse(
            &serde_json::json!({"limit": 10_000, "cursor": "2"}),
            DEFAULT_PAGE_LIMIT,
            3,
        )
        .unwrap();
        let page = request.page(vec![0, 1, 2, 3, 4, 5]).unwrap();

        assert_eq!(page.items, vec![2, 3, 4]);
        assert_eq!(page.next_cursor.as_deref(), Some("5"));
        assert_eq!(page.metadata()["limit_clamped"], true);
    }

    #[test]
    fn invalid_cursor_is_rejected() {
        let error = PageRequest::parse(
            &serde_json::json!({"cursor": "later"}),
            DEFAULT_PAGE_LIMIT,
            MAX_PAGE_LIMIT,
        )
        .unwrap_err();
        assert!(error.contains("cursor"));
    }

    #[test]
    fn empty_cursor_is_the_first_page() {
        let request = PageRequest::parse(
            &serde_json::json!({"cursor": ""}),
            DEFAULT_PAGE_LIMIT,
            MAX_PAGE_LIMIT,
        )
        .unwrap();

        assert_eq!(request.offset, 0);
    }
}
