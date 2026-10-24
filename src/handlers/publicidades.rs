use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::handlers::empresas::verificar_dueno;
use crate::handlers::moderacion::exigir_superadmin;
use crate::state::AppState;
use crate::validation::url_http;

/// `id` viaja como texto porque así lo modela el frontend (`Publicidad.id: string`).
/// `empresa_id` es `NULL` en las colectas/eventos que difunde la plataforma (RF-06).
#[derive(FromRow, Serialize)]
pub struct Publicidad {
    id: String,
    empresa_id: Option<i32>,
    titulo: String,
    descripcion: String,
    imagen_url: String,
}

const SELECT: &str =
    "SELECT id::text AS id, empresa_id, titulo, COALESCE(descripcion, '') AS descripcion, \
                      COALESCE(imagen_url, '') AS imagen_url FROM publicaciones_destacadas \
                      WHERE activa";

#[derive(Deserialize)]
pub struct Filtro {
    empresa_id: Option<i32>,
}

pub async fn listar(
    State(state): State<AppState>,
    Query(f): Query<Filtro>,
) -> AppResult<Json<Vec<Publicidad>>> {
    let rows = sqlx::query_as::<_, Publicidad>(&format!(
        "{SELECT} AND ($1::int4 IS NULL OR empresa_id = $1) ORDER BY id DESC LIMIT 60"
    ))
    .bind(f.empresa_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct NuevaPublicidad {
    /// `None` = colecta/evento de la plataforma (solo superadmin); `Some` = comercio propio.
    empresa_id: Option<i32>,
    titulo: String,
    descripcion: Option<String>,
    imagen_url: Option<String>,
}

pub async fn crear(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<NuevaPublicidad>,
) -> AppResult<(StatusCode, Json<Publicidad>)> {
    if body.titulo.trim().is_empty() {
        return Err(AppError::BadRequest("El título es obligatorio".into()));
    }
    // RF-06: la publicación se compone de imagen, título y descripción breve.
    let descripcion = body
        .descripcion
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("La descripción es obligatoria".into()))?;
    let imagen_url = body
        .imagen_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("La imagen es obligatoria".into()))?;
    url_http("imagen_url", imagen_url)?;
    match body.empresa_id {
        Some(empresa_id) => verificar_dueno(&state, &user, empresa_id).await?,
        None => exigir_superadmin(&user)?,
    }
    let fila = sqlx::query_as::<_, Publicidad>(
        "INSERT INTO publicaciones_destacadas (empresa_id, titulo, descripcion, imagen_url) VALUES ($1, $2, $3, $4) \
         RETURNING id::text AS id, empresa_id, titulo, descripcion, imagen_url",
    )
    .bind(body.empresa_id)
    .bind(body.titulo.trim())
    .bind(descripcion)
    .bind(imagen_url)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(fila)))
}

pub async fn eliminar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    let empresa_id = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT empresa_id FROM publicaciones_destacadas WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("Publicidad no encontrada".into()))?;
    match empresa_id {
        Some(eid) => verificar_dueno(&state, &user, eid).await?,
        None if user.rol != "superadmin" => {
            return Err(AppError::Forbidden(
                "Solo la plataforma puede borrar esta publicación".into(),
            ))
        }
        None => {}
    }
    sqlx::query("DELETE FROM publicaciones_destacadas WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "message": "Publicidad eliminada" })))
}
