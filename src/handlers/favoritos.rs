use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppResult};
use crate::state::AppState;

pub async fn listar(user: CurrentUser, State(state): State<AppState>) -> AppResult<Json<Vec<i32>>> {
    let ids = sqlx::query_scalar::<_, i32>(
        "SELECT empresa_id FROM favoritos WHERE usuario_id = $1 ORDER BY creado_en",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(ids))
}

pub async fn agregar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(empresa_id): Path<i32>,
) -> AppResult<Json<Value>> {
    sqlx::query(
        "INSERT INTO favoritos (usuario_id, empresa_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(user.id)
    .bind(empresa_id)
    .execute(&state.db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23503") => {
            AppError::NotFound("Empresa no encontrada".into())
        }
        _ => AppError::Db(e),
    })?;
    Ok(Json(json!({ "message": "Agregado a favoritos" })))
}

pub async fn quitar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(empresa_id): Path<i32>,
) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM favoritos WHERE usuario_id = $1 AND empresa_id = $2")
        .bind(user.id)
        .bind(empresa_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "message": "Quitado de favoritos" })))
}
