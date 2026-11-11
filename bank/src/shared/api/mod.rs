//! The Open Banking-shaped response envelope (spec: Public interface).
//! `api_tran_dtm` is the system time of the response, as at the KFTC gateway.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Serialize;
use ulid::Ulid;

use crate::shared::clock::kst;
use crate::shared::context::RequestContext;

/// Open Banking API response codes used by the bank (`rsp_code`).
pub mod codes {
    pub const SUCCESS: &str = "A0000";
    pub const SYSTEM_ERROR: &str = "A0001";
    pub const INVALID_PARAMETER: &str = "A0002";
    pub const NOT_FOUND: &str = "A0003";
    pub const DUPLICATE_REQUEST: &str = "A0007";
    pub const BUSY: &str = "A0013";
    pub const UNAUTHORIZED: &str = "O0001";
}

#[derive(Debug, Serialize)]
pub struct Envelope<T: Serialize> {
    pub api_tran_id: String,
    pub api_tran_dtm: String,
    pub rsp_code: &'static str,
    pub rsp_message: String,
    pub request_id: String,
    #[serde(flatten)]
    pub body: T,
}

fn api_tran_dtm() -> String {
    Utc::now()
        .with_timezone(&kst())
        .format("%Y%m%d%H%M%S%3f")
        .to_string()
}

/// A successful response.
pub fn ok<T: Serialize>(ctx: &RequestContext, body: T) -> Response {
    let env = Envelope {
        api_tran_id: Ulid::new().to_string(),
        api_tran_dtm: api_tran_dtm(),
        rsp_code: codes::SUCCESS,
        rsp_message: String::new(),
        request_id: ctx.request_id.clone(),
        body,
    };
    (StatusCode::OK, Json(env)).into_response()
}

/// An error response with an Open Banking response code and a Korean message.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub rsp_code: &'static str,
    pub rsp_message: String,
    pub request_id: String,
}

impl ApiError {
    pub fn new(
        ctx: &RequestContext,
        status: StatusCode,
        rsp_code: &'static str,
        rsp_message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            rsp_code,
            rsp_message: rsp_message.into(),
            request_id: ctx.request_id.clone(),
        }
    }

    pub fn internal(ctx: &RequestContext) -> Self {
        Self::new(
            ctx,
            StatusCode::INTERNAL_SERVER_ERROR,
            codes::SYSTEM_ERROR,
            "시스템 오류가 발생했습니다",
        )
    }

    pub fn busy(ctx: &RequestContext) -> Self {
        Self::new(
            ctx,
            StatusCode::SERVICE_UNAVAILABLE,
            codes::BUSY,
            "잠시 후 다시 시도해 주세요",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let env = Envelope {
            api_tran_id: Ulid::new().to_string(),
            api_tran_dtm: api_tran_dtm(),
            rsp_code: self.rsp_code,
            rsp_message: self.rsp_message,
            request_id: self.request_id,
            body: serde_json::Map::new(),
        };
        (self.status, Json(env)).into_response()
    }
}
