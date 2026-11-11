//! Internal interface for the generator (spec: Internal interface). Mounted
//! only on the generator listener. Stage 1: clock read; seeding is step 5.

use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;

use crate::app::AppState;
use crate::shared::api::{self, ApiError};
use crate::shared::context::RequestContext;

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/v1/clock", get(read_clock))
}

async fn read_clock(
    State(state): State<AppState>,
    ctx: RequestContext,
) -> Result<Response, ApiError> {
    let clock = state.clock.state().await.map_err(|e| {
        tracing::error!(request_id = %ctx.request_id, error = %e, "clock read failed");
        ApiError::internal(&ctx)
    })?;
    Ok(api::ok(&ctx, clock))
}
