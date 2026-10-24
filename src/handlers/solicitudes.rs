//! RF-06: canal público para pedir difusión (publicidad de la plataforma o colectas/eventos
//! solidarios) sin tener cuenta, con una bandeja para que el superadmin le dé curso.
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use std::time::Duration;

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::handlers::moderacion::exigir_superadmin;
use crate::rate_limit::{espera_legible, ClientIp};
use crate::state::AppState;

const TIPOS: [&str; 2] = ["publicidad", "colecta"];
const ESTADOS: [&str; 3] = ["pendiente", "atendida", "descartada"];
const VENTANA_SOLICITUDES: Duration = Duration::from_secs(60 * 60);

#[derive(Deserialize)]
pub struct NuevaSolicitud {
    nombre: String,
    contacto: String,
    tipo: Option<String>,
    mensaje: String,
}

#[derive(FromRow, Serialize)]
pub struct SolicitudPublicidad {
    id: i32,
    nombre: String,
    contacto: String,
    tipo: String,
    mensaje: String,
    estado: String,
    creado_en: DateTime<Utc>,
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

/// `POST /solicitudes-publicidad`: público, sin sesión. El contacto puede ser email o teléfono.
pub async fn crear(
    ClientIp(ip): ClientIp,
    State(state): State<AppState>,
    AppJson(body): AppJson<NuevaSolicitud>,
) -> AppResult<(StatusCode, Json<Value>)> {
    // Canal abierto: 10 pedidos por IP y por hora alcanzan de sobra para una persona.
    let clave = format!("solicitud-publicidad:{ip}");
    if let Some(espera) = state.limiter.bloqueado(&clave, 10, VENTANA_SOLICITUDES) {
        return Err(AppError::TooManyRequests(format!(
            "Demasiados pedidos desde esta conexión. Probá de nuevo en {}.",
            espera_legible(espera)
        )));
    }
    let nombre = body.nombre.trim();
    texto_entre("El nombre", nombre, 2, 100)?;
    let contacto = body.contacto.trim();
    texto_entre("El contacto", contacto, 5, 150)?;
    let digitos = contacto.chars().filter(|c| c.is_ascii_digit()).count();
    if !contacto.contains('@') && digitos < 6 {
        return Err(AppError::BadRequest(
            "Dejá un email o un teléfono de contacto".into(),
        ));
    }
    let tipo = body.tipo.as_deref().unwrap_or("publicidad");
    if !TIPOS.contains(&tipo) {
        return Err(AppError::BadRequest(
            "El tipo debe ser 'publicidad' o 'colecta'".into(),
        ));
    }
    let mensaje = body.mensaje.trim();
    texto_entre("El mensaje", mensaje, 10, 1000)?;
    state.limiter.registrar(&clave);

    // Evita que un mismo contacto llene la bandeja repitiendo la misma solicitud.
    let pendiente: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM solicitudes_publicidad WHERE lower(contacto) = lower($1) AND estado = 'pendiente')",
    )
    .bind(contacto)
    .fetch_one(&state.db)
    .await?;
    if pendiente {
        return Err(AppError::Conflict(
            "Ya recibimos una solicitud con este contacto y está pendiente".into(),
        ));
    }

    sqlx::query("INSERT INTO solicitudes_publicidad (nombre, contacto, tipo, mensaje) VALUES ($1, $2, $3, $4)")
        .bind(nombre)
        .bind(contacto)
        .bind(tipo)
        .bind(mensaje)
        .execute(&state.db)
        .await?;
    let cuerpo = format!("{nombre} envió una solicitud de {tipo}");
    state
        .push
        .enviar_a_rol(
            &state.db,
            "superadmin",
            &crate::push::Notificacion {
                titulo: "Nueva solicitud de difusión",
                cuerpo: &cuerpo,
                url: "/admin",
            },
        )
        .await;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({ "message": "Recibimos tu solicitud. La plataforma se va a contactar con vos." }),
        ),
    ))
}

#[derive(Deserialize)]
pub struct FiltroSolicitudes {
    /// `true` para ver también las atendidas y descartadas; por defecto solo las pendientes.
    todas: Option<bool>,
}

pub async fn listar(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(filtro): Query<FiltroSolicitudes>,
) -> AppResult<Json<Vec<SolicitudPublicidad>>> {
    exigir_superadmin(&user)?;
    let rows = sqlx::query_as::<_, SolicitudPublicidad>(
        "SELECT id, nombre, contacto, tipo, mensaje, estado, creado_en FROM solicitudes_publicidad \
         WHERE ($1 OR estado = 'pendiente') ORDER BY estado, creado_en DESC LIMIT 200",
    )
    .bind(filtro.todas.unwrap_or(false))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct EstadoRequest {
    estado: String,
}

pub async fn actualizar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<EstadoRequest>,
) -> AppResult<Json<Value>> {
    exigir_superadmin(&user)?;
    if !ESTADOS.contains(&body.estado.as_str()) {
        return Err(AppError::BadRequest(
            "El estado debe ser 'pendiente', 'atendida' o 'descartada'".into(),
        ));
    }
    let filas = sqlx::query("UPDATE solicitudes_publicidad SET estado = $2 WHERE id = $1")
        .bind(id)
        .bind(&body.estado)
        .execute(&state.db)
        .await?
        .rows_affected();
    if filas == 0 {
        return Err(AppError::NotFound("Solicitud no encontrada".into()));
    }
    Ok(Json(
        json!({ "message": "Solicitud actualizada", "estado": body.estado }),
    ))
}
