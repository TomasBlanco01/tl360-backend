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

#[derive(FromRow, Serialize)]
pub struct Categoria {
    id: i32,
    nombre: String,
}

/// Listado enriquecido: súper rubro, si algún comercio del rubro toma turnos (el front separa
/// "con reserva" de "solo directorio" con este dato) y cuántos comercios tiene.
#[derive(FromRow, Serialize)]
pub struct CategoriaListado {
    id: i32,
    nombre: String,
    super_rubro_id: Option<i32>,
    super_rubro: Option<String>,
    turnos_habilitados: bool,
    cantidad: i64,
}

#[derive(Deserialize)]
pub struct CategoriaRequest {
    nombre: String,
}

pub async fn get_categorias(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<CategoriaListado>>> {
    let rows = sqlx::query_as::<_, CategoriaListado>(
        "SELECT c.id, c.nombre, c.super_rubro_id, sr.nombre AS super_rubro,          EXISTS (SELECT 1 FROM empresa_categorias ec JOIN empresas e ON e.id = ec.empresa_id                  WHERE ec.categoria_id = c.id AND e.turnos_habilitados) AS turnos_habilitados,          (SELECT COUNT(*) FROM empresa_categorias ec WHERE ec.categoria_id = c.id) AS cantidad          FROM categorias c LEFT JOIN super_rubros sr ON sr.id = c.super_rubro_id ORDER BY c.nombre",
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

pub async fn crear_categoria(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<CategoriaRequest>,
) -> AppResult<(StatusCode, Json<Categoria>)> {
    require_role(&user, &["superadmin"])?;
    let nombre = body.nombre.trim();
    if nombre.is_empty() {
        return Err(AppError::BadRequest("El nombre es obligatorio".into()));
    }
    let row = sqlx::query_as::<_, Categoria>(
        "INSERT INTO categorias (nombre) VALUES ($1) RETURNING id, nombre",
    )
    .bind(nombre)
    .fetch_one(&state.db)
    .await
    .map_err(nombre_repetido)?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn actualizar_categoria(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<CategoriaRequest>,
) -> AppResult<Json<Categoria>> {
    require_role(&user, &["superadmin"])?;
    let nombre = body.nombre.trim();
    if nombre.is_empty() {
        return Err(AppError::BadRequest("El nombre es obligatorio".into()));
    }
    let row = sqlx::query_as::<_, Categoria>(
        "UPDATE categorias SET nombre = $1 WHERE id = $2 RETURNING id, nombre",
    )
    .bind(nombre)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(nombre_repetido)?
    .ok_or(AppError::NotFound("Categoría no encontrada".into()))?;
    Ok(Json(row))
}

pub async fn eliminar_categoria(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    require_role(&user, &["superadmin"])?;
    let result = sqlx::query("DELETE FROM categorias WHERE id = $1")
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
        return Err(AppError::NotFound("Categoría no encontrada".into()));
    }
    Ok(Json(
        json!({ "mensaje": "Categoría eliminada correctamente" }),
    ))
}
