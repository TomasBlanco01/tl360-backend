use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::agenda::ahora_local;
use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::handlers::empresas::verificar_dueno;
use crate::state::AppState;

const MAX_CANTIDAD: i32 = 1000;
const MAX_IMPORTE: f64 = 99_999_999.99;
const MAX_NOTA: usize = 300;

#[derive(FromRow, Serialize)]
pub struct AtencionMostrador {
    id: i32,
    empresa_id: i32,
    fecha: NaiveDate,
    cantidad: i32,
    /// Pesos con centavos; en la base se guarda como `importe_centavos` para evitar decimales binarios.
    importe: Option<f64>,
    nota: Option<String>,
    creado_en: DateTime<Utc>,
}

#[derive(FromRow, Serialize)]
pub struct ResumenAtenciones {
    hoy_cantidad: i64,
    hoy_importe: f64,
    mes_cantidad: i64,
    mes_importe: f64,
    total_cantidad: i64,
    total_importe: f64,
}

#[derive(Serialize)]
pub struct AtencionesDelComercio {
    atenciones: Vec<AtencionMostrador>,
    resumen: ResumenAtenciones,
}

const SELECT_ATENCION: &str =
    "SELECT id, empresa_id, fecha, cantidad, importe_centavos::float8 / 100 AS importe, nota, creado_en \
     FROM atenciones_mostrador";

pub async fn listar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(empresa_id): Path<i32>,
) -> AppResult<Json<AtencionesDelComercio>> {
    verificar_dueno(&state, &user, empresa_id).await?;
    let hoy = ahora_local().date();

    let atenciones = sqlx::query_as::<_, AtencionMostrador>(&format!(
        "{SELECT_ATENCION} WHERE empresa_id = $1 ORDER BY fecha DESC, id DESC LIMIT 100"
    ))
    .bind(empresa_id)
    .fetch_all(&state.db)
    .await?;

    let resumen = sqlx::query_as::<_, ResumenAtenciones>(
        "SELECT \
           COALESCE(SUM(cantidad) FILTER (WHERE fecha = $2), 0)::int8 AS hoy_cantidad, \
           COALESCE(SUM(importe_centavos) FILTER (WHERE fecha = $2), 0)::float8 / 100 AS hoy_importe, \
           COALESCE(SUM(cantidad) FILTER (WHERE date_trunc('month', fecha) = date_trunc('month', $2::date)), 0)::int8 AS mes_cantidad, \
           COALESCE(SUM(importe_centavos) FILTER (WHERE date_trunc('month', fecha) = date_trunc('month', $2::date)), 0)::float8 / 100 AS mes_importe, \
           COALESCE(SUM(cantidad), 0)::int8 AS total_cantidad, \
           COALESCE(SUM(importe_centavos), 0)::float8 / 100 AS total_importe \
         FROM atenciones_mostrador WHERE empresa_id = $1",
    )
    .bind(empresa_id)
    .bind(hoy)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(AtencionesDelComercio {
        atenciones,
        resumen,
    }))
}

#[derive(Deserialize)]
pub struct NuevaAtencion {
    /// Si falta se usa la fecha local de hoy.
    fecha: Option<NaiveDate>,
    cantidad: i32,
    /// Importe manual en pesos; `None`/0 = atención sin movimiento de dinero registrado.
    importe: Option<f64>,
    nota: Option<String>,
}

pub async fn crear(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(empresa_id): Path<i32>,
    AppJson(body): AppJson<NuevaAtencion>,
) -> AppResult<(StatusCode, Json<AtencionMostrador>)> {
    verificar_dueno(&state, &user, empresa_id).await?;

    if !(1..=MAX_CANTIDAD).contains(&body.cantidad) {
        return Err(AppError::BadRequest(format!(
            "La cantidad debe ser un entero entre 1 y {MAX_CANTIDAD}"
        )));
    }
    let hoy = ahora_local().date();
    let fecha = body.fecha.unwrap_or(hoy);
    if fecha > hoy {
        return Err(AppError::BadRequest(
            "No se pueden registrar atenciones futuras".into(),
        ));
    }
    let nota = body
        .nota
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    if nota
        .as_deref()
        .is_some_and(|n| n.chars().count() > MAX_NOTA)
    {
        return Err(AppError::BadRequest(format!(
            "La nota no puede superar los {MAX_NOTA} caracteres"
        )));
    }
    let importe_centavos = match body.importe {
        None => None,
        Some(importe) => {
            if !importe.is_finite() || importe <= 0.0 || importe > MAX_IMPORTE {
                return Err(AppError::BadRequest(
                    "El importe debe ser mayor a cero y menor a 100.000.000".into(),
                ));
            }
            Some((importe * 100.0).round() as i64)
        }
    };

    let fila = sqlx::query_as::<_, AtencionMostrador>(&format!(
        "INSERT INTO atenciones_mostrador (empresa_id, fecha, cantidad, importe_centavos, nota, creado_por) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, empresa_id, fecha, cantidad, importe_centavos::float8 / 100 AS importe, nota, creado_en"
    ))
    .bind(empresa_id)
    .bind(fecha)
    .bind(body.cantidad)
    .bind(importe_centavos)
    .bind(nota)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(fila)))
}

pub async fn eliminar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((empresa_id, atencion_id)): Path<(i32, i32)>,
) -> AppResult<Json<Value>> {
    verificar_dueno(&state, &user, empresa_id).await?;
    let borradas =
        sqlx::query("DELETE FROM atenciones_mostrador WHERE id = $1 AND empresa_id = $2")
            .bind(atencion_id)
            .bind(empresa_id)
            .execute(&state.db)
            .await?
            .rows_affected();
    if borradas == 0 {
        return Err(AppError::NotFound("Atención no encontrada".into()));
    }
    Ok(Json(json!({ "message": "Atención eliminada" })))
}
