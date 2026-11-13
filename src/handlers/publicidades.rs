//! RF-06: publicidades de los comercios en el carrusel de portada. Nacen `pendiente` y solo se
//! muestran cuando el superadmin las aprueba (tabla estado_aprobacion) y están dentro de sus fechas.
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::handlers::empresas::verificar_dueno;
use crate::handlers::moderacion::exigir_superadmin;
use crate::state::AppState;
use crate::validation::{texto_hasta, url_http};

/// Estados del circuito de aprobación (filas de estado_aprobacion), compartidos con las difusiones.
pub(crate) const ESTADOS_APROBACION: [&str; 4] = ["pendiente", "aprobada", "rechazada", "finalizada"];

pub(crate) fn estado_valido(estado: &str) -> AppResult<&str> {
    ESTADOS_APROBACION
        .iter()
        .find(|e| **e == estado.trim())
        .copied()
        .ok_or_else(|| {
            AppError::BadRequest(
                "El estado debe ser 'pendiente', 'aprobada', 'rechazada' o 'finalizada'".into(),
            )
        })
}

/// Fechas de vigencia opcionales: si están las dos, el fin no puede ser anterior al inicio.
pub(crate) fn validar_vigencia(inicio: Option<NaiveDate>, fin: Option<NaiveDate>) -> AppResult<()> {
    if let (Some(i), Some(f)) = (inicio, fin) {
        if f < i {
            return Err(AppError::BadRequest(
                "La fecha de fin no puede ser anterior a la de inicio".into(),
            ));
        }
    }
    Ok(())
}

/// `id` viaja como texto porque así lo modela el frontend (`Publicidad.id: string`).
#[derive(FromRow, Serialize)]
pub struct Publicidad {
    id: String,
    empresa_id: i32,
    empresa_nombre: String,
    empresa_slug: Option<String>,
    titulo: String,
    descripcion: String,
    url_imagen: String,
    estado: String,
    fecha_inicio: Option<NaiveDate>,
    fecha_fin: Option<NaiveDate>,
}

const SELECT: &str = "SELECT p.id::text AS id, p.empresa_id, e.nombre AS empresa_nombre, \
     e.slug AS empresa_slug, p.titulo, p.descripcion, p.url_imagen, ea.nombre AS estado, \
     p.fecha_inicio, p.fecha_fin \
     FROM publicidad p JOIN empresa e ON e.id = p.empresa_id \
     JOIN estado_aprobacion ea ON ea.id = p.estado_aprobacion_id";

/// Aprobada y dentro de sus fechas (las que no tienen fechas valen siempre).
pub(crate) const VIGENTE: &str = "ea.nombre = 'aprobada' \
     AND (fecha_inicio IS NULL OR fecha_inicio <= $9::date) \
     AND (fecha_fin IS NULL OR fecha_fin >= $9::date)";

#[derive(Deserialize)]
pub struct Filtro {
    empresa_id: Option<i32>,
    /// Gestión: `pendiente`, `aprobada`, …, o `todas`. Sin este filtro solo se listan las vigentes.
    estado: Option<String>,
    /// Gestión: busca por nombre del comercio o por título (el superadmin no conoce los ids).
    q: Option<String>,
}

/// Público: las vigentes. Con `estado`, el superadmin ve la bandeja y el dueño, con `empresa_id`,
/// todas las de su comercio (para saber cuáles esperan aprobación o fueron rechazadas).
pub async fn listar(
    user: Option<CurrentUser>,
    State(state): State<AppState>,
    Query(f): Query<Filtro>,
) -> AppResult<Json<Vec<Publicidad>>> {
    let hoy = crate::agenda::ahora_local().date();
    let Some(estado) = f.estado.as_deref().map(str::trim).filter(|e| !e.is_empty()) else {
        let rows = sqlx::query_as::<_, Publicidad>(&format!(
            "{SELECT} WHERE {} AND ($1::int4 IS NULL OR p.empresa_id = $1) ORDER BY p.id DESC LIMIT 60",
            VIGENTE.replace("$9", "$2")
        ))
        .bind(f.empresa_id)
        .bind(hoy)
        .fetch_all(&state.db)
        .await?;
        return Ok(Json(rows));
    };
    let user = user.ok_or(AppError::Unauthorized)?;
    match f.empresa_id {
        Some(empresa_id) => verificar_dueno(&state, &user, empresa_id).await?,
        None => exigir_superadmin(&user)?,
    }
    let estado = if estado == "todas" { None } else { Some(estado_valido(estado)?) };
    let q = f
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);
    let rows = sqlx::query_as::<_, Publicidad>(&format!(
        "{SELECT} WHERE ($1::int4 IS NULL OR p.empresa_id = $1) AND ($2::text IS NULL OR ea.nombre = $2) \
         AND ($3::text IS NULL \
              OR translate(lower(e.nombre), 'áéíóúüñ', 'aeiouun') LIKE '%' || translate($3, 'áéíóúüñ', 'aeiouun') || '%' \
              OR translate(lower(p.titulo), 'áéíóúüñ', 'aeiouun') LIKE '%' || translate($3, 'áéíóúüñ', 'aeiouun') || '%') \
         ORDER BY p.estado_aprobacion_id, p.id DESC LIMIT 200"
    ))
    .bind(f.empresa_id)
    .bind(estado)
    .bind(q)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct NuevaPublicidad {
    empresa_id: i32,
    titulo: String,
    descripcion: Option<String>,
    url_imagen: Option<String>,
    fecha_inicio: Option<NaiveDate>,
    fecha_fin: Option<NaiveDate>,
}

/// El dueño la carga y queda pendiente de aprobación; si la carga el superadmin, queda aprobada.
pub async fn crear(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<NuevaPublicidad>,
) -> AppResult<(StatusCode, Json<Publicidad>)> {
    let titulo = body.titulo.trim();
    if titulo.is_empty() {
        return Err(AppError::BadRequest("El título es obligatorio".into()));
    }
    texto_hasta("El título", titulo, 100)?;
    // RF-06: la publicidad se compone de imagen, título y descripción breve.
    let descripcion = body
        .descripcion
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("La descripción es obligatoria".into()))?;
    texto_hasta("La descripción", descripcion, 500)?;
    let url_imagen = body
        .url_imagen
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("La imagen es obligatoria".into()))?;
    url_http("url_imagen", url_imagen)?;
    validar_vigencia(body.fecha_inicio, body.fecha_fin)?;
    verificar_dueno(&state, &user, body.empresa_id).await?;
    let estado = if user.rol == "superadmin" { "aprobada" } else { "pendiente" };

    let id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO publicidad (empresa_id, titulo, descripcion, url_imagen, fecha_inicio, fecha_fin, estado_aprobacion_id) \
         VALUES ($1, $2, $3, $4, $5, $6, (SELECT id FROM estado_aprobacion WHERE nombre = $7)) RETURNING id",
    )
    .bind(body.empresa_id)
    .bind(titulo)
    .bind(descripcion)
    .bind(url_imagen)
    .bind(body.fecha_inicio)
    .bind(body.fecha_fin)
    .bind(estado)
    .fetch_one(&state.db)
    .await?;
    if estado == "pendiente" {
        crate::handlers::difusiones::avisar_superadmins(
            &state,
            "Publicidad para aprobar",
            &format!("Un comercio cargó «{titulo}»"),
        )
        .await;
    }
    Ok((StatusCode::CREATED, Json(una(&state, id).await?)))
}

async fn una(state: &AppState, id: i32) -> AppResult<Publicidad> {
    sqlx::query_as::<_, Publicidad>(&format!("{SELECT} WHERE p.id = $1"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("Publicidad no encontrada".into()))
}

#[derive(Deserialize)]
pub struct EstadoRequest {
    estado: String,
}

/// El superadmin aprueba, rechaza o finaliza. El dueño solo puede darla de baja (finalizada) o
/// reenviarla a revisión (pendiente) después de un rechazo.
pub async fn cambiar_estado(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<EstadoRequest>,
) -> AppResult<Json<Publicidad>> {
    let estado = estado_valido(&body.estado)?;
    let publicidad = una(&state, id).await?;
    if user.rol != "superadmin" {
        verificar_dueno(&state, &user, publicidad.empresa_id).await?;
        let permitido = estado == "finalizada"
            || (estado == "pendiente" && publicidad.estado == "rechazada");
        if !permitido {
            return Err(AppError::sin_permisos_por("solo el superadmin aprueba o rechaza publicidades"));
        }
    }
    sqlx::query(
        "UPDATE publicidad SET estado_aprobacion_id = (SELECT id FROM estado_aprobacion WHERE nombre = $2) WHERE id = $1",
    )
    .bind(id)
    .bind(estado)
    .execute(&state.db)
    .await?;
    Ok(Json(una(&state, id).await?))
}

pub async fn eliminar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    let empresa_id =
        sqlx::query_scalar::<_, i32>("SELECT empresa_id FROM publicidad WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound("Publicidad no encontrada".into()))?;
    verificar_dueno(&state, &user, empresa_id).await?;
    sqlx::query("DELETE FROM publicidad WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "message": "Publicidad eliminada" })))
}
