//! Webhook delivery ledger persistence.

use canary_core::query::{
    DEFAULT_WEBHOOK_DELIVERY_LIMIT, MAX_WEBHOOK_DELIVERY_LIMIT, WebhookDeliveriesResponse,
    WebhookDelivery, WebhookDeliveryCursor, decode_webhook_delivery_cursor,
    encode_webhook_delivery_cursor, webhook_deliveries_response,
};
use rusqlite::{Connection, params};

use crate::Result;

/// Result type returned by webhook delivery read models.
pub type WebhookDeliveryPageResult<T> = std::result::Result<T, WebhookDeliveryPageError>;

/// Webhook delivery page validation or storage failure.
#[derive(Debug, thiserror::Error)]
pub enum WebhookDeliveryPageError {
    /// Limit is not a positive integer up to the Phoenix maximum.
    #[error("invalid webhook delivery limit")]
    InvalidLimit,
    /// Cursor is not a valid Phoenix webhook delivery cursor.
    #[error("invalid webhook delivery cursor")]
    InvalidCursor,
    /// Status is not one of the Phoenix ledger statuses.
    #[error("invalid webhook delivery status")]
    InvalidStatus,
    /// SQLite rejected the read.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Delivery status values accepted by the Phoenix schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookDeliveryStatus {
    /// Delivery has been enqueued but not attempted.
    Pending,
    /// Delivery has failed at least once and can be retried.
    Retrying,
    /// Delivery succeeded.
    Delivered,
    /// Delivery failed permanently or could not be used.
    Discarded,
    /// Delivery was intentionally not sent.
    Suppressed,
}

impl WebhookDeliveryStatus {
    /// Return the persisted status string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Retrying => "retrying",
            Self::Delivered => "delivered",
            Self::Discarded => "discarded",
            Self::Suppressed => "suppressed",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "retrying" => Self::Retrying,
            "delivered" => Self::Delivered,
            "discarded" => Self::Discarded,
            "suppressed" => Self::Suppressed,
            _ => Self::Pending,
        }
    }

    /// Parse a user-supplied Phoenix status filter.
    pub fn parse_filter(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "retrying" => Some(Self::Retrying),
            "delivered" => Some(Self::Delivered),
            "discarded" => Some(Self::Discarded),
            "suppressed" => Some(Self::Suppressed),
            _ => None,
        }
    }
}

/// Return Phoenix's accepted webhook delivery statuses in wire order.
pub const fn statuses() -> &'static [&'static str] {
    &[
        "pending",
        "retrying",
        "delivered",
        "discarded",
        "suppressed",
    ]
}

/// Ledger fields required to create a pending or suppressed delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookDeliveryInsert {
    /// Stable delivery id.
    pub delivery_id: String,
    /// Webhook subscription id.
    pub webhook_id: String,
    /// Event name.
    pub event: String,
    /// RFC3339 timestamp.
    pub now: String,
}

/// Delivery ledger row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookDeliveryRow {
    /// Stable delivery id.
    pub delivery_id: String,
    /// Webhook subscription id.
    pub webhook_id: String,
    /// Event name.
    pub event: String,
    /// Current status.
    pub status: WebhookDeliveryStatus,
    /// Number of HTTP attempts.
    pub attempt_count: i64,
    /// Discard/suppression reason.
    pub reason: Option<String>,
    /// First attempt timestamp.
    pub first_attempt_at: Option<String>,
    /// Last attempt timestamp.
    pub last_attempt_at: Option<String>,
    /// Success timestamp.
    pub delivered_at: Option<String>,
    /// Permanent-discard timestamp.
    pub discarded_at: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Webhook delivery list filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookDeliveryListOptions {
    /// Optional delivery id filter.
    pub delivery_id: Option<String>,
    /// Optional webhook id filter.
    pub webhook_id: Option<String>,
    /// Optional event filter.
    pub event: Option<String>,
    /// Optional status filter.
    pub status: Option<WebhookDeliveryStatus>,
    /// Maximum rows to return.
    pub limit: Option<u32>,
}

/// Optional filters for the public webhook delivery page route.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookDeliveryPageOptions {
    /// Optional webhook id filter.
    pub webhook_id: Option<String>,
    /// Optional event filter.
    pub event: Option<String>,
    /// Optional status filter.
    pub status: Option<String>,
    /// Maximum rows to return.
    pub limit: Option<String>,
    /// Pagination cursor.
    pub cursor: Option<String>,
}

/// Active webhook subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookSubscription {
    /// Webhook id.
    pub id: String,
    /// Destination URL.
    pub url: String,
    /// JSON-encoded subscribed event names.
    pub events: String,
    /// Shared secret.
    pub secret: String,
    /// Whether the subscription is active.
    pub active: bool,
    /// RFC3339 creation timestamp.
    pub created_at: String,
}

/// Webhook subscription row to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookSubscriptionInsert {
    /// Webhook id.
    pub id: String,
    /// Destination URL.
    pub url: String,
    /// Subscribed events.
    pub events: Vec<String>,
    /// Shared secret.
    pub secret: String,
    /// Whether the subscription is active.
    pub active: bool,
    /// RFC3339 creation timestamp.
    pub created_at: String,
}

impl WebhookSubscription {
    /// Return true when this subscription includes `event`.
    pub fn subscribes_to(&self, event: &str) -> bool {
        serde_json::from_str::<Vec<String>>(&self.events)
            .is_ok_and(|events| events.iter().any(|subscribed| subscribed == event))
    }
}

pub(crate) fn insert_subscription(
    connection: &mut Connection,
    subscription: WebhookSubscriptionInsert,
) -> Result<()> {
    connection.execute(
        "INSERT INTO webhooks (id, url, events, secret, active, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            subscription.id,
            subscription.url,
            serde_json::to_string(&subscription.events)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            subscription.secret,
            if subscription.active { 1 } else { 0 },
            subscription.created_at,
        ],
    )?;
    Ok(())
}

pub(crate) fn list_subscriptions(connection: &Connection) -> Result<Vec<WebhookSubscription>> {
    let mut statement = connection.prepare(
        "SELECT id, url, events, secret, active, created_at
         FROM webhooks
         ORDER BY created_at, id",
    )?;
    let rows = statement.query_map([], subscription_row)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub(crate) fn delete_subscription(connection: &mut Connection, webhook_id: &str) -> Result<bool> {
    let changed = connection.execute("DELETE FROM webhooks WHERE id = ?1", [webhook_id])?;
    Ok(changed > 0)
}

pub(crate) fn create_pending(
    connection: &mut Connection,
    delivery: WebhookDeliveryInsert,
) -> Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO webhook_deliveries (
            delivery_id, webhook_id, event, status, attempt_count, created_at, updated_at
         ) VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?4)",
        params![
            delivery.delivery_id,
            delivery.webhook_id,
            delivery.event,
            delivery.now,
        ],
    )?;
    Ok(())
}

pub(crate) fn create_suppressed(
    connection: &mut Connection,
    delivery: WebhookDeliveryInsert,
    reason: &str,
) -> Result<()> {
    connection.execute(
        "INSERT INTO webhook_deliveries (
            delivery_id, webhook_id, event, status, attempt_count, reason, created_at, updated_at
         ) VALUES (?1, ?2, ?3, 'suppressed', 0, ?4, ?5, ?5)
         ON CONFLICT(delivery_id) DO UPDATE SET
            status = 'suppressed',
            reason = excluded.reason,
            updated_at = excluded.updated_at",
        params![
            delivery.delivery_id,
            delivery.webhook_id,
            delivery.event,
            reason,
            delivery.now,
        ],
    )?;
    Ok(())
}

pub(crate) fn mark_attempt(
    connection: &mut Connection,
    delivery_id: &str,
    now: &str,
) -> Result<()> {
    connection.execute(
        "UPDATE webhook_deliveries
         SET status = CASE
                WHEN status IN ('pending', 'retrying') THEN 'retrying'
                ELSE status
             END,
             attempt_count = attempt_count + 1,
             first_attempt_at = COALESCE(first_attempt_at, ?2),
             last_attempt_at = ?2,
             updated_at = ?2
         WHERE delivery_id = ?1",
        params![delivery_id, now],
    )?;
    Ok(())
}

pub(crate) fn mark_delivered(
    connection: &mut Connection,
    delivery_id: &str,
    now: &str,
) -> Result<()> {
    connection.execute(
        "UPDATE webhook_deliveries
         SET status = 'delivered', delivered_at = ?2, updated_at = ?2
         WHERE delivery_id = ?1",
        params![delivery_id, now],
    )?;
    Ok(())
}

pub(crate) fn mark_discarded(
    connection: &mut Connection,
    delivery_id: &str,
    reason: &str,
    now: &str,
) -> Result<()> {
    connection.execute(
        "UPDATE webhook_deliveries
         SET status = 'discarded', reason = ?2, discarded_at = ?3, updated_at = ?3
         WHERE delivery_id = ?1",
        params![delivery_id, reason, now],
    )?;
    Ok(())
}

pub(crate) fn list(
    connection: &Connection,
    options: WebhookDeliveryListOptions,
) -> Result<Vec<WebhookDeliveryRow>> {
    let mut sql = String::from(
        "SELECT delivery_id, webhook_id, event, status, attempt_count, reason,
                first_attempt_at, last_attempt_at, delivered_at, discarded_at,
                created_at, updated_at
         FROM webhook_deliveries WHERE 1 = 1",
    );
    let mut filters = Vec::new();

    if let Some(delivery_id) = options.delivery_id {
        sql.push_str(" AND delivery_id = ?");
        filters.push(delivery_id);
    }
    if let Some(webhook_id) = options.webhook_id {
        sql.push_str(" AND webhook_id = ?");
        filters.push(webhook_id);
    }
    if let Some(event) = options.event {
        sql.push_str(" AND event = ?");
        filters.push(event);
    }
    if let Some(status) = options.status {
        sql.push_str(" AND status = ?");
        filters.push(status.as_str().to_owned());
    }

    sql.push_str(" ORDER BY created_at DESC, delivery_id DESC LIMIT ?");
    filters.push(options.limit.unwrap_or(50).to_string());

    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(filters), row)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub(crate) fn page(
    connection: &Connection,
    options: WebhookDeliveryPageOptions,
) -> WebhookDeliveryPageResult<WebhookDeliveriesResponse> {
    let limit = parse_page_limit(options.limit.as_deref())?;
    let cursor = parse_page_cursor(options.cursor.as_deref())?;
    let status = parse_page_status(options.status.as_deref())?;

    let mut sql = String::from(
        "SELECT delivery_id, webhook_id, event, status, attempt_count, reason,
                first_attempt_at, last_attempt_at, delivered_at, discarded_at,
                created_at, updated_at
         FROM webhook_deliveries WHERE 1 = 1",
    );
    let mut filters = Vec::new();

    if let Some(webhook_id) = options.webhook_id.filter(|value| !value.is_empty()) {
        sql.push_str(" AND webhook_id = ?");
        filters.push(webhook_id);
    }
    if let Some(event) = options.event.filter(|value| !value.is_empty()) {
        sql.push_str(" AND event = ?");
        filters.push(event);
    }
    if let Some(status) = status {
        sql.push_str(" AND status = ?");
        filters.push(status.as_str().to_owned());
    }
    if let Some(cursor) = cursor {
        sql.push_str(" AND (created_at < ? OR (created_at = ? AND delivery_id < ?))");
        filters.push(cursor.created_at.clone());
        filters.push(cursor.created_at);
        filters.push(cursor.delivery_id);
    }

    sql.push_str(" ORDER BY created_at DESC, delivery_id DESC LIMIT ?");
    filters.push((limit + 1).to_string());

    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(filters), row)?;
    let mut rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    let cursor = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().and_then(|last| {
            encode_webhook_delivery_cursor(&WebhookDeliveryCursor {
                created_at: last.created_at.clone(),
                delivery_id: last.delivery_id.clone(),
            })
        })
    } else {
        None
    };
    let deliveries = rows.into_iter().map(format_delivery).collect();

    Ok(webhook_deliveries_response(deliveries, cursor))
}

pub(crate) fn active_subscriptions_for_event(
    connection: &Connection,
    event: &str,
) -> Result<Vec<WebhookSubscription>> {
    let mut statement = connection.prepare(
        "SELECT id, url, events, secret, active, created_at
         FROM webhooks
         WHERE active = 1
         ORDER BY created_at, id",
    )?;
    let rows = statement.query_map([], subscription_row)?;

    let subscriptions = rows
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|subscription| subscription.subscribes_to(event))
        .collect();
    Ok(subscriptions)
}

pub(crate) fn subscription_by_id(
    connection: &Connection,
    webhook_id: &str,
) -> Result<Option<WebhookSubscription>> {
    let mut statement = connection.prepare(
        "SELECT id, url, events, secret, active, created_at
         FROM webhooks
         WHERE id = ?1",
    )?;
    let result = statement.query_row([webhook_id], subscription_row);

    match result {
        Ok(subscription) => Ok(Some(subscription)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn subscription_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebhookSubscription> {
    Ok(WebhookSubscription {
        id: row.get(0)?,
        url: row.get(1)?,
        events: row.get(2)?,
        secret: row.get(3)?,
        active: row.get::<_, i64>(4)? == 1,
        created_at: row.get(5)?,
    })
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebhookDeliveryRow> {
    let status: String = row.get(3)?;
    Ok(WebhookDeliveryRow {
        delivery_id: row.get(0)?,
        webhook_id: row.get(1)?,
        event: row.get(2)?,
        status: WebhookDeliveryStatus::from_str(&status),
        attempt_count: row.get(4)?,
        reason: row.get(5)?,
        first_attempt_at: row.get(6)?,
        last_attempt_at: row.get(7)?,
        delivered_at: row.get(8)?,
        discarded_at: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn parse_page_limit(limit: Option<&str>) -> WebhookDeliveryPageResult<usize> {
    match limit {
        None | Some("") => Ok(DEFAULT_WEBHOOK_DELIVERY_LIMIT),
        Some(limit) => match limit.parse::<usize>() {
            Ok(limit) if (1..=MAX_WEBHOOK_DELIVERY_LIMIT).contains(&limit) => Ok(limit),
            _ => Err(WebhookDeliveryPageError::InvalidLimit),
        },
    }
}

fn parse_page_cursor(
    cursor: Option<&str>,
) -> WebhookDeliveryPageResult<Option<WebhookDeliveryCursor>> {
    match cursor {
        None | Some("") => Ok(None),
        Some(cursor) => decode_webhook_delivery_cursor(cursor)
            .map(Some)
            .ok_or(WebhookDeliveryPageError::InvalidCursor),
    }
}

fn parse_page_status(
    status: Option<&str>,
) -> WebhookDeliveryPageResult<Option<WebhookDeliveryStatus>> {
    match status {
        None | Some("") => Ok(None),
        Some(status) => WebhookDeliveryStatus::parse_filter(status)
            .map(Some)
            .ok_or(WebhookDeliveryPageError::InvalidStatus),
    }
}

fn format_delivery(row: WebhookDeliveryRow) -> WebhookDelivery {
    let completed_at = row
        .delivered_at
        .clone()
        .or_else(|| row.discarded_at.clone())
        .or_else(|| {
            if matches!(
                row.status,
                WebhookDeliveryStatus::Suppressed
                    | WebhookDeliveryStatus::Discarded
                    | WebhookDeliveryStatus::Delivered
            ) {
                Some(row.updated_at.clone())
            } else {
                None
            }
        });

    WebhookDelivery {
        delivery_id: row.delivery_id,
        webhook_id: row.webhook_id,
        event: row.event,
        status: row.status.as_str().to_owned(),
        attempt_count: row.attempt_count,
        reason: row.reason,
        first_attempt_at: row.first_attempt_at,
        last_attempt_at: row.last_attempt_at,
        delivered_at: row.delivered_at,
        discarded_at: row.discarded_at,
        completed_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}
