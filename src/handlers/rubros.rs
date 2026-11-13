use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth_extractor::{require_role, CurrentUser};
use crate::errors::{AppError, AppJson, AppResult};
use crate::state::AppState;
use crate::validation::texto_hasta;

#[derive(FromRow, Serialize)]
pub struct Rubro {
    id: i32,
    nombre: String,
}

/// Listado enriquecido: si algún comercio del rubro toma turnos (el front separa "con reserva" de
/// "solo directorio" con este dato) y cuántos comercios tiene.
#[derive(FromRow, Serialize)]
pub struct RubroListado {
    id: i32,
    nombre: String,
    turnos_habilitados: bool,
    cantidad: i64,
}

#[derive(Deserialize)]
pub struct RubroRequest {
    nombre: String,
}

pub async fn get_rubros(State(state): State<AppState>) -> AppResult<Json<Vec<RubroListado>>> {
    let rows = sqlx::query_as::<_, RubroListado>(
        "SELECT r.id, r.nombre, \
                EXISTS (SELECT 1 FROM empresa e WHERE e.rubro_id = r.id AND e.turnos_habilitados) AS turnos_habilitados, \
                (SELECT COUNT(*) FROM empresa e WHERE e.rubro_id = r.id) AS cantidad \
         FROM rubro r ORDER BY r.nombre",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

fn nombre_repetido(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => {
            AppError::Conflict("Ya existe un rubro con ese nombre".into())
        }
        _ => AppError::Db(e),
    }
}

fn nombre_valido(body: &RubroRequest) -> AppResult<&str> {
    let nombre = body.nombre.trim();
    if nombre.is_empty() {
        return Err(AppError::BadRequest("El nombre es obligatorio".into()));
    }
    texto_hasta("El nombre", nombre, 80)?;
    Ok(nombre)
}

pub async fn crear_rubro(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<RubroRequest>,
) -> AppResult<(StatusCode, Json<Rubro>)> {
    require_role(&user, &["superadmin"])?;
    let nombre = nombre_valido(&body)?;
    let row = sqlx::query_as::<_, Rubro>("INSERT INTO rubro (nombre) VALUES ($1) RETURNING id, nombre")
        .bind(nombre)
        .fetch_one(&state.db)
        .await
        .map_err(nombre_repetido)?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn actualizar_rubro(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<RubroRequest>,
) -> AppResult<Json<Rubro>> {
    require_role(&user, &["superadmin"])?;
    let nombre = nombre_valido(&body)?;
    let row = sqlx::query_as::<_, Rubro>(
        "UPDATE rubro SET nombre = $1 WHERE id = $2 RETURNING id, nombre",
    )
    .bind(nombre)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(nombre_repetido)?
    .ok_or(AppError::NotFound("Rubro no encontrado".into()))?;
    Ok(Json(row))
}

pub async fn eliminar_rubro(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    require_role(&user, &["superadmin"])?;
    let result = sqlx::query("DELETE FROM rubro WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.code().as_deref() == Some("23503") => AppError::Conflict(
                "El rubro tiene comercios asignados y no puede eliminarse".into(),
            ),
            _ => AppError::Db(e),
        })?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Rubro no encontrado".into()));
    }
    Ok(Json(json!({ "mensaje": "Rubro eliminado correctamente" })))
}
