//! Analyst authentication and read-only bank views.
mod customers;
mod login;
mod views;

use axum::{Router, http::StatusCode, response::Response, routing::post};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth;
use crate::shared::context::RequestContext;
use crate::shared::db::DbError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/v1/login", post(login::login))
        .merge(customers::router())
        .merge(views::router())
}

pub fn seed_router() -> Router<AppState> {
    Router::new().route("/internal/v1/seed/analysts", post(login::seed))
}

fn invalid(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        "요청 항목 오류",
    )
}

fn not_found(ctx: &RequestContext) -> ApiError {
    ApiError::new(ctx, StatusCode::NOT_FOUND, codes::NOT_FOUND, "대상 없음")
}

fn db_error(ctx: &RequestContext, error: &DbError) -> ApiError {
    tracing::warn!(request_id = %ctx.request_id, error = %error, "analyst database operation failed");
    if matches!(error, DbError::Busy) || matches!(error.ora_code(), Some(60 | 30006)) {
        ApiError::busy(ctx)
    } else {
        ApiError::internal(ctx)
    }
}

fn parse_row(row: &str) -> Result<Value, DbError> {
    serde_json::from_str(row).map_err(|error| DbError::Config(format!("analyst row JSON: {error}")))
}

#[derive(Debug, Default, Deserialize)]
struct PageInput {
    pub limit: Option<u16>,
    pub cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    binding: String,
    first: String,
    second: String,
}

struct Page {
    limit: usize,
    binding: String,
    cursor: Option<Cursor>,
}

impl Page {
    fn new(ctx: &RequestContext, input: PageInput, binding: Value) -> Result<Self, ApiError> {
        let limit = usize::from(input.limit.unwrap_or(25));
        if !(1..=100).contains(&limit) {
            return Err(invalid(ctx));
        }
        let binding = auth::hex(&Sha256::digest(binding.to_string()));
        let cursor = input
            .cursor
            .map(|encoded| {
                if encoded.len() > 1024 {
                    return Err(invalid(ctx));
                }
                let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid(ctx))?;
                let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid(ctx))?;
                if cursor.binding != binding {
                    return Err(invalid(ctx));
                }
                Ok(cursor)
            })
            .transpose()?;
        Ok(Self {
            limit,
            binding,
            cursor,
        })
    }

    fn take(&self) -> i64 {
        (self.limit + 1) as i64
    }

    fn cursor(&self) -> Option<(&str, &str)> {
        self.cursor
            .as_ref()
            .map(|key| (key.first.as_str(), key.second.as_str()))
    }

    fn response(
        self,
        ctx: &RequestContext,
        mut rows: Vec<(String, String, String)>,
    ) -> Result<Response, ApiError> {
        let has_more = rows.len() > self.limit;
        rows.truncate(self.limit);
        let next = if has_more {
            let (first, second, _) = rows.last().expect("a nonzero page with more rows");
            let cursor = Cursor {
                binding: self.binding,
                first: first.clone(),
                second: second.clone(),
            };
            let bytes = serde_json::to_vec(&cursor).expect("a cursor contains only strings");
            Some(URL_SAFE_NO_PAD.encode(bytes))
        } else {
            None
        };
        let items = rows
            .iter()
            .map(|(_, _, row)| parse_row(row))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_error(ctx, &e))?;
        Ok(api::ok(ctx, json!({"items": items, "next_cursor": next})))
    }
}
