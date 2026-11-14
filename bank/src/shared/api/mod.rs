//! The Open Banking-shaped response envelope (spec: Public interface).
//! `api_tran_dtm` is the system time of the response, as at the KFTC gateway.

use axum::Json as AxumJson;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Serialize;
use ulid::Ulid;

use crate::shared::clock::kst;
use crate::shared::context::RequestContext;

/// Open Banking API response codes used by the bank (`rsp_code`).
pub mod codes {
    /// Every code the bank can return, for replaying stored outcomes.
    pub fn known(code: &str) -> Option<&'static str> {
        [
            SUCCESS,
            SYSTEM_ERROR,
            INVALID_PARAMETER,
            NOT_FOUND,
            BANK_REJECTED,
            DUPLICATE_REQUEST,
            BUSY,
            UNAUTHORIZED,
        ]
        .into_iter()
        .find(|c| *c == code)
    }

    pub const SUCCESS: &str = "A0000";
    pub const SYSTEM_ERROR: &str = "A0001";
    pub const INVALID_PARAMETER: &str = "A0002";
    pub const NOT_FOUND: &str = "A0003";
    pub const BANK_REJECTED: &str = "A0004";
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
    metadata_response(StatusCode::OK, codes::SUCCESS, &env.body, AxumJson(&env))
}

/// A response with an explicit HTTP status and response code (used to replay
/// stored outcomes and to report bank-level rejections with their details).
pub fn respond<T: Serialize>(
    ctx: &RequestContext,
    status: StatusCode,
    rsp_code: &'static str,
    rsp_message: impl Into<String>,
    body: T,
) -> Response {
    let env = Envelope {
        api_tran_id: Ulid::new().to_string(),
        api_tran_dtm: api_tran_dtm(),
        rsp_code,
        rsp_message: rsp_message.into(),
        request_id: ctx.request_id.clone(),
        body,
    };
    metadata_response(status, rsp_code, &env.body, AxumJson(&env))
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
        metadata_response(self.status, self.rsp_code, &env.body, AxumJson(&env))
    }
}

/// Open Banking field formats shared by features.
pub mod fields {
    use chrono::{DateTime, FixedOffset};

    use crate::shared::clock::kst;

    /// Saltation's synthetic `bank_code_std` (contracts/codes/institutions.json).
    pub const BANK_CODE: &str = "998";
    pub const BANK_NAME: &str = "살테이션은행";

    /// `account_num_masked`: the last three digits replaced with `*`.
    pub fn mask_account(account_no: &str) -> String {
        let keep = account_no.len().saturating_sub(3);
        format!("{}***", &account_no[..keep])
    }

    /// Open Banking `account_type`: 1 demand, 2 term deposit, 4 loan.
    pub fn account_type(subtype: &str) -> &'static str {
        match subtype {
            "TERM" => "2",
            "LOAN" => "4",
            _ => "1",
        }
    }

    /// `YYYYMMDD` in KST.
    pub fn date(t: DateTime<FixedOffset>) -> String {
        t.with_timezone(&kst()).format("%Y%m%d").to_string()
    }

    /// `HHMMSS` in KST.
    pub fn time(t: DateTime<FixedOffset>) -> String {
        t.with_timezone(&kst()).format("%H%M%S").to_string()
    }

    /// A valid `bank_tran_id`: AN(20), the 11th character `U` or `M`.
    pub fn valid_bank_tran_id(id: &str) -> bool {
        id.len() == 20
            && id.chars().all(|c| c.is_ascii_alphanumeric())
            && matches!(id.as_bytes()[10], b'U' | b'M')
    }

    /// A valid `tran_dtime`: N(14).
    pub fn valid_tran_dtime(v: &str) -> bool {
        v.len() == 14 && v.chars().all(|c| c.is_ascii_digit())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn formats() {
            assert_eq!(mask_account("99801234567"), "99801234***");
            assert_eq!(account_type("DEMAND"), "1");
            assert!(valid_bank_tran_id("M202601234U000000001"));
            assert!(!valid_bank_tran_id("M202601234X000000001"));
            assert!(valid_tran_dtime("20260103091244"));
        }
    }
}

fn metadata_response<T: Serialize>(
    status: StatusCode,
    code: &str,
    body: &T,
    content: impl IntoResponse,
) -> Response {
    let value = serde_json::to_value(body).unwrap_or_default();
    let bank_code = value
        .get("bank_rsp_code")
        .or_else(|| value.get("res_list")?.get(0)?.get("bank_rsp_code"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let mut response = (status, content).into_response();
    response
        .extensions_mut()
        .insert(crate::shared::outcome::ResponseMetadata {
            rsp_code: Some(code.to_owned()),
            bank_rsp_code: bank_code,
            ..Default::default()
        });
    response
}

fn invalid_extractor(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        "요청 항목 오류",
    )
}

macro_rules! parts_extractor {
    ($name:ident) => {
        pub struct $name<T>(pub T);
        impl<S, T> axum::extract::FromRequestParts<S> for $name<T>
        where
            S: Send + Sync,
            T: serde::de::DeserializeOwned + Send,
        {
            type Rejection = ApiError;
            async fn from_request_parts(
                parts: &mut axum::http::request::Parts,
                state: &S,
            ) -> Result<Self, Self::Rejection> {
                let ctx = RequestContext::from_request_parts(parts, state)
                    .await
                    .unwrap_or_else(|e| match e {});
                axum::extract::$name::<T>::from_request_parts(parts, state)
                    .await
                    .map(|v| Self(v.0))
                    .map_err(|_| invalid_extractor(&ctx))
            }
        }
    };
}
parts_extractor!(Query);
parts_extractor!(Path);

macro_rules! body_extractor {
    ($name:ident) => {
        pub struct $name<T>(pub T);
        impl<S, T> axum::extract::FromRequest<S> for $name<T>
        where
            S: Send + Sync,
            T: serde::de::DeserializeOwned + Send,
        {
            type Rejection = ApiError;
            async fn from_request(
                req: axum::extract::Request,
                state: &S,
            ) -> Result<Self, Self::Rejection> {
                let (mut parts, body) = req.into_parts();
                let ctx =
                    <RequestContext as axum::extract::FromRequestParts<S>>::from_request_parts(
                        &mut parts, state,
                    )
                    .await
                    .unwrap_or_else(|e| match e {});
                axum::$name::<T>::from_request(
                    axum::extract::Request::from_parts(parts, body),
                    state,
                )
                .await
                .map(|v| Self(v.0))
                .map_err(|_| invalid_extractor(&ctx))
            }
        }
    };
}
body_extractor!(Json);
body_extractor!(Form);
