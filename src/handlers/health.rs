use std::time::Duration;

use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};

use crate::state::AppState;

const DB_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let consulta = tokio::time::timeout(
        DB_TIMEOUT,
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.db),
    )
    .await;

    match consulta {
        Ok(Ok(_)) => (StatusCode::OK, Json(json!({ "status": "ok", "db": "ok" }))),
        Ok(Err(e)) => {
            tracing::warn!("health: la base de datos no responde: {e}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "degraded", "db": "error" })),
            )
        }
        Err(_) => {
            tracing::warn!(
                "health: la base de datos tardó más de {} s",
                DB_TIMEOUT.as_secs()
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "degraded", "db": "timeout" })),
            )
        }
    }
}

pub async fn live() -> &'static str {
    "ok"
}
