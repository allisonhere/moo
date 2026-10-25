//! Cursor pagination over the lists a snapshot already filtered for the actor, so there is exactly
//! one place that decides who may see what.
//!
//! A cursor names the last item returned, by a key that never changes for that item: messages and
//! decisions are immutable and ordered by `(revision, id)`; tasks and approvals change over time,
//! so they are ordered by `id`. Paging therefore neither skips nor repeats an item when other things
//! change between requests.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{Error, Result};

pub(super) const DEFAULT_LIMIT: usize = 50;
pub(super) const MAX_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Order {
    /// `(revision, id)`: immutable records in the order they happened.
    Chronological,
    /// `id`: records whose other fields change.
    ById,
}

#[derive(Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ListParams {
    #[serde(default)]
    pub team_id: Option<String>,
    /// Messages only: restrict to one task's thread.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Tasks and approvals only: restrict to one status tag (e.g. `pending`).
    #[serde(default)]
    pub status: Option<String>,
    /// The `next_cursor` of the previous page; omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

fn key(item: &Value, order: Order) -> (u64, String) {
    let id = item["id"].as_str().unwrap_or_default().to_owned();
    match order {
        Order::Chronological => (item["revision"].as_u64().unwrap_or(0), id),
        Order::ById => (0, id),
    }
}

fn encode(item: &Value, order: Order) -> String {
    let (revision, id) = key(item, order);
    match order {
        Order::Chronological => format!("{revision}.{id}"),
        Order::ById => id,
    }
}

fn decode(cursor: &str, order: Order) -> Result<(u64, String)> {
    let bad = || Error::new("invalid_cursor", "that cursor was not issued for this list");
    match order {
        Order::ById => Ok((0, cursor.to_owned())),
        Order::Chronological => {
            let (revision, id) = cursor.split_once('.').ok_or_else(bad)?;
            Ok((revision.parse().map_err(|_| bad())?, id.to_owned()))
        }
    }
}

/// One page of `items` (already visible to the caller and in `order`).
pub(super) fn page(items: &[Value], order: Order, params: &ListParams) -> Result<Value> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(Error::new(
            "invalid_params",
            format!("limit must be between 1 and {MAX_LIMIT}"),
        ));
    }
    let after = params
        .cursor
        .as_deref()
        .map(|c| decode(c, order))
        .transpose()?;
    let mut rest = items
        .iter()
        .filter(|item| after.as_ref().is_none_or(|after| key(item, order) > *after));
    let page: Vec<&Value> = rest.by_ref().take(limit).collect();
    let has_more = rest.next().is_some();
    Ok(json!({
        "items": page,
        "has_more": has_more,
        "next_cursor": if has_more { page.last().map(|item| encode(item, order)) } else { None },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: u64) -> Vec<Value> {
        (1..=n)
            .map(|i| json!({"id": format!("m{i:03}"), "revision": i * 2}))
            .collect()
    }

    fn walk(items: &[Value], order: Order, limit: usize) -> Vec<String> {
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let params = ListParams {
                cursor: cursor.clone(),
                limit: Some(limit),
                ..Default::default()
            };
            let page = page(items, order, &params).unwrap();
            seen.extend(
                page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["id"].as_str().unwrap().to_owned()),
            );
            if !page["has_more"].as_bool().unwrap() {
                assert!(page["next_cursor"].is_null());
                return seen;
            }
            cursor = page["next_cursor"].as_str().map(str::to_owned);
        }
    }

    #[test]
    fn paging_returns_every_item_once_in_order_for_any_page_size() {
        let all = items(25);
        let expected: Vec<String> = all
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect();
        for order in [Order::Chronological, Order::ById] {
            for limit in [1, 3, 7, 25, 100] {
                assert_eq!(walk(&all, order, limit), expected, "{order:?} {limit}");
            }
        }
    }

    #[test]
    fn new_items_after_the_cursor_appear_and_nothing_repeats() {
        let mut all = items(10);
        let first = page(
            &all,
            Order::Chronological,
            &ListParams {
                limit: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        let cursor = first["next_cursor"].as_str().unwrap().to_owned();
        all.push(json!({"id": "m011", "revision": 22}));
        let rest = page(
            &all,
            Order::Chronological,
            &ListParams {
                cursor: Some(cursor),
                limit: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
        let ids: Vec<&str> = rest["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            ["m005", "m006", "m007", "m008", "m009", "m010", "m011"]
        );
    }

    #[test]
    fn bad_limits_and_cursors_are_typed_errors() {
        let all = items(3);
        for limit in [0, 101] {
            let params = ListParams {
                limit: Some(limit),
                ..Default::default()
            };
            assert_eq!(
                page(&all, Order::ById, &params).unwrap_err().code,
                "invalid_params"
            );
        }
        let params = ListParams {
            cursor: Some("garbage".into()),
            ..Default::default()
        };
        assert_eq!(
            page(&all, Order::Chronological, &params).unwrap_err().code,
            "invalid_cursor"
        );
        let params = ListParams {
            cursor: Some("x.m1".into()),
            ..Default::default()
        };
        assert_eq!(
            page(&all, Order::Chronological, &params).unwrap_err().code,
            "invalid_cursor"
        );
    }

    #[test]
    fn ids_containing_dots_survive_the_round_trip() {
        let all = vec![
            json!({"id": "a.b.c", "revision": 5}),
            json!({"id": "a.b.d", "revision": 5}),
        ];
        let first = page(
            &all,
            Order::Chronological,
            &ListParams {
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        let cursor = first["next_cursor"].as_str().unwrap();
        assert_eq!(cursor, "5.a.b.c");
        let second = page(
            &all,
            Order::Chronological,
            &ListParams {
                cursor: Some(cursor.into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(second["items"][0]["id"], "a.b.d");
    }
}
