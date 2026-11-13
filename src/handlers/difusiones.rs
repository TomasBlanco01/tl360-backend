//! RF-06: difusión de colectas y eventos solidarios. Cualquiera la pide sin cuenta (canal público)
//! y queda `pendiente` hasta que el superadmin la aprueba; las que carga el superadmin nacen
//! aprobadas. Incluye las colectas de la plataforma y los pedidos públicos de difusión.
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use std::time::Duration;

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::handlers::moderacion::exigir_superadmin;
use crate::handlers::publicidades::{estado_valido, validar_vigencia};
use crate::rate_limit::{espera_legible, ClientIp};
use crate::state::AppState;
use crate::validation::url_http_opt;

const TIPOS: [&str; 2] = ["colecta", "evento"];
const VENTANA_PEDIDOS: Duration = Duration::from_secs(60 * 60);

#[derive(FromRow, Serialize)]
pub struct Difusion {
    id: i32,
    tipo: String,
    titulo: String,
    descripcion: String,
    url_imagen: Option<String>,
    /// Datos de quien pidió la difusión: solo los ven el superadmin y el propio solicitante al
    /// crearla. La portada pública no los incluye.
    #[serde(skip_serializing_if = "Option::is_none")]
    solicitante_nombre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    solicitante_contacto: Option<String>,
    estado: String,
    fecha_inicio: Option<NaiveDate>,
    fecha_fin: Option<NaiveDate>,
    creado_en: DateTime<Utc>,
}

const SELECT: &str = "SELECT d.id, d.tipo, d.titulo, d.descripcion, d.url_imagen, d.solicitante_nombre, \
     d.solicitante_contacto, ea.nombre AS estado, d.fecha_inicio, d.fecha_fin, d.creado_en \
     FROM difusion d JOIN estado_aprobacion ea ON ea.id = d.estado_aprobacion_id";

#[derive(Deserialize)]
pub struct NuevaDifusion {
    tipo: Option<String>,
    titulo: String,
    descripcion: String,
    url_imagen: Option<String>,
    /// Obligatorios en el canal público; el superadmin puede omitirlos (se usan sus datos).
    solicitante_nombre: Option<String>,
    solicitante_contacto: Option<String>,
    fecha_inicio: Option<NaiveDate>,
    fecha_fin: Option<NaiveDate>,
}

fn texto_entre(campo: &str, valor: &str, min: usize, max: usize) -> AppResult<()> {
    let largo = valor.chars().count();
    if !(min..=max).contains(&largo) {
        return Err(AppError::BadRequest(format!(
            "{campo} debe tener entre {min} y {max} caracteres"
        )));
    }
    Ok(())
}

pub(crate) async fn avisar_superadmins(state: &AppState, titulo: &str, cuerpo: &str) {
    state
        .push
        .enviar_a_rol(
            &state.db,
            "superadmin",
            &crate::push::Notificacion {
                titulo,
                cuerpo,
                url: "/admin",
            },
        )
        .await;
}

/// `POST /difusiones`: público, sin sesión (con límite por IP). Con sesión de superadmin, la
/// difusión de la plataforma queda aprobada directamente.
pub async fn crear(
    user: Option<CurrentUser>,
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    AppJson(body): AppJson<NuevaDifusion>,
) -> AppResult<(StatusCode, Json<Difusion>)> {
    let superadmin = user.as_ref().filter(|u| u.rol == "superadmin");
    // Canal abierto: 10 pedidos por IP y por hora alcanzan de sobra para una persona.
    let clave = format!("difusion:{ip}");
    if superadmin.is_none() {
        if let Some(espera) = state.limiter.bloqueado(&clave, 10, VENTANA_PEDIDOS) {
            return Err(AppError::TooManyRequests(format!(
                "Demasiados pedidos desde esta conexión. Probá de nuevo en {}.",
                espera_legible(espera)
            )));
        }
    }
    let tipo = body.tipo.as_deref().map(str::trim).unwrap_or("colecta");
    if !TIPOS.contains(&tipo) {
        return Err(AppError::BadRequest(
            "El tipo debe ser 'colecta' o 'evento'".into(),
        ));
    }
    let titulo = body.titulo.trim();
    texto_entre("El título", titulo, 3, 100)?;
    let descripcion = body.descripcion.trim();
    texto_entre("La descripción", descripcion, 10, 1000)?;
    url_http_opt("url_imagen", &body.url_imagen)?;
    validar_vigencia(body.fecha_inicio, body.fecha_fin)?;
    let url_imagen = body.url_imagen.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let (nombre, contacto) = match superadmin {
        Some(u) => {
            let email = sqlx::query_scalar::<_, String>("SELECT email FROM usuario WHERE id = $1")
                .bind(u.id)
                .fetch_one(&state.db)
                .await?;
            (
                body.solicitante_nombre.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&u.nombre).to_string(),
                body.solicitante_contacto.as_deref().map(str::trim).filter(|s| !s.is_empty()).map_or(email, str::to_string),
            )
        }
        None => {
            let nombre = body.solicitante_nombre.as_deref().map(str::trim).unwrap_or_default();
            texto_entre("El nombre", nombre, 2, 100)?;
            let contacto = body.solicitante_contacto.as_deref().map(str::trim).unwrap_or_default();
            texto_entre("El contacto", contacto, 5, 150)?;
            let digitos = contacto.chars().filter(|c| c.is_ascii_digit()).count();
            if !contacto.contains('@') && digitos < 6 {
                return Err(AppError::BadRequest(
                    "Dejá un email o un teléfono de contacto".into(),
                ));
            }
            (nombre.to_string(), contacto.to_string())
        }
    };
    let estado = if superadmin.is_some() { "aprobada" } else { "pendiente" };
    if superadmin.is_none() {
        state.limiter.registrar(&clave);
    }

    // difusion_pendiente_uq impide dos pedidos pendientes del mismo contacto, también ante
    // envíos simultáneos.
    let id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO difusion (usuario_id, estado_aprobacion_id, tipo, titulo, descripcion, url_imagen, \
         solicitante_nombre, solicitante_contacto, fecha_inicio, fecha_fin) \
         VALUES ($1, (SELECT id FROM estado_aprobacion WHERE nombre = $2), $3, $4, $5, $6, $7, $8, $9, $10) \
         RETURNING id",
    )
    .bind(user.as_ref().map(|u| u.id))
    .bind(estado)
    .bind(tipo)
    .bind(titulo)
    .bind(descripcion)
    .bind(url_imagen)
    .bind(&nombre)
    .bind(&contacto)
    .bind(body.fecha_inicio)
    .bind(body.fecha_fin)
    .fetch_one(&state.db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => AppError::Conflict(
            "Ya recibimos un pedido con este contacto y está pendiente".into(),
        ),
        _ => AppError::Db(e),
    })?;
    if estado == "pendiente" {
        avisar_superadmins(
            &state,
            "Nuevo pedido de difusión",
            &format!("{nombre} pidió difundir «{titulo}»"),
        )
        .await;
    }
    Ok((StatusCode::CREATED, Json(una(&state, id).await?)))
}

async fn una(state: &AppState, id: i32) -> AppResult<Difusion> {
    sqlx::query_as::<_, Difusion>(&format!("{SELECT} WHERE d.id = $1"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("Difusión no encontrada".into()))
}

#[derive(Deserialize)]
pub struct Filtro {
    /// Gestión (superadmin): `pendiente`, `aprobada`, …, o `todas`. Sin filtro, las vigentes.
    estado: Option<String>,
}

pub async fn listar(
    user: Option<CurrentUser>,
    State(state): State<AppState>,
    Query(f): Query<Filtro>,
) -> AppResult<Json<Vec<Difusion>>> {
    let Some(estado) = f.estado.as_deref().map(str::trim).filter(|e| !e.is_empty()) else {
        let rows = sqlx::query_as::<_, Difusion>(&format!(
            "{SELECT} WHERE ea.nombre = 'aprobada' \
             AND (d.fecha_inicio IS NULL OR d.fecha_inicio <= $1) \
             AND (d.fecha_fin IS NULL OR d.fecha_fin >= $1) \
             ORDER BY d.id DESC LIMIT 30"
        ))
        .bind(crate::agenda::ahora_local().date())
        .fetch_all(&state.db)
        .await?;
        let publicas = rows
            .into_iter()
            .map(|d| Difusion { solicitante_nombre: None, solicitante_contacto: None, ..d })
            .collect();
        return Ok(Json(publicas));
    };
    let user = user.ok_or(AppError::Unauthorized)?;
    exigir_superadmin(&user)?;
    let estado = if estado == "todas" { None } else { Some(estado_valido(estado)?) };
    let rows = sqlx::query_as::<_, Difusion>(&format!(
        "{SELECT} WHERE ($1::text IS NULL OR ea.nombre = $1) \
         ORDER BY d.estado_aprobacion_id, d.creado_en DESC LIMIT 200"
    ))
    .bind(estado)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct EstadoRequest {
    estado: String,
}

pub async fn cambiar_estado(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<EstadoRequest>,
) -> AppResult<Json<Difusion>> {
    exigir_superadmin(&user)?;
    let estado = estado_valido(&body.estado)?;
    let filas = sqlx::query(
        "UPDATE difusion SET estado_aprobacion_id = (SELECT id FROM estado_aprobacion WHERE nombre = $2) WHERE id = $1",
    )
    .bind(id)
    .bind(estado)
    .execute(&state.db)
    .await
    .map_err(|e| match &e {
        // Volver a pendiente cuando ya hay otro pedido pendiente del mismo contacto.
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => AppError::Conflict(
            "Ya hay otro pedido pendiente con este contacto".into(),
        ),
        _ => AppError::Db(e),
    })?
    .rows_affected();
    if filas == 0 {
        return Err(AppError::NotFound("Difusión no encontrada".into()));
    }
    Ok(Json(una(&state, id).await?))
}

pub async fn eliminar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    exigir_superadmin(&user)?;
    let filas = sqlx::query("DELETE FROM difusion WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if filas == 0 {
        return Err(AppError::NotFound("Difusión no encontrada".into()));
    }
    Ok(Json(json!({ "message": "Difusión eliminada" })))
}
