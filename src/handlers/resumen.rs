//! Resumen de actividad del comercio. Se calcula a pedido para el período que elija el dueño (una
//! semana, un mes o cualquier rango) y el panel lo descarga en PDF; no se guarda en ninguna tabla.
use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppResult};
use crate::handlers::empresas::verificar_dueno;
use crate::state::AppState;

/// Un año alcanza para cualquier informe del panel y acota el costo de la consulta.
const MAX_DIAS: i64 = 366;

#[derive(Deserialize)]
pub struct Periodo {
    desde: Option<NaiveDate>,
    hasta: Option<NaiveDate>,
}

#[derive(FromRow, Serialize)]
pub struct Totales {
    turnos: i64,
    online: i64,
    mostrador: i64,
    confirmados: i64,
    completados: i64,
    cancelados: i64,
    no_asistio: i64,
}

#[derive(FromRow, Serialize)]
pub struct TurnosDelDia {
    fecha: NaiveDate,
    turnos: i64,
    cancelados: i64,
}

#[derive(Serialize)]
pub struct Resumen {
    empresa_id: i32,
    empresa_nombre: String,
    desde: NaiveDate,
    hasta: NaiveDate,
    turnos: Totales,
    calificaciones: i64,
    calificacion_promedio: Option<f64>,
    favoritos_nuevos: i64,
    por_dia: Vec<TurnosDelDia>,
}

/// Sin fechas: los últimos 30 días hasta hoy. Las fechas son inclusivas.
fn periodo(p: &Periodo, hoy: NaiveDate) -> AppResult<(NaiveDate, NaiveDate)> {
    let hasta = p.hasta.unwrap_or(hoy);
    let desde = p.desde.unwrap_or(hasta - Duration::days(29));
    if hasta < desde {
        return Err(AppError::BadRequest(
            "La fecha de fin no puede ser anterior a la de inicio".into(),
        ));
    }
    if (hasta - desde).num_days() >= MAX_DIAS {
        return Err(AppError::BadRequest(
            "El período no puede superar un año".into(),
        ));
    }
    Ok((desde, hasta))
}

pub async fn get_resumen(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Query(p): Query<Periodo>,
) -> AppResult<Json<Resumen>> {
    verificar_dueno(&state, &user, id).await?;
    let (desde, hasta) = periodo(&p, crate::agenda::ahora_local().date())?;

    let empresa_nombre = sqlx::query_scalar::<_, String>("SELECT nombre FROM empresa WHERE id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let turnos = sqlx::query_as::<_, Totales>(
        "SELECT COUNT(*) FILTER (WHERE et.nombre <> 'cancelado') AS turnos, \
                COUNT(*) FILTER (WHERE et.nombre <> 'cancelado' AND t.origen = 'online') AS online, \
                COUNT(*) FILTER (WHERE et.nombre <> 'cancelado' AND t.origen = 'mostrador') AS mostrador, \
                COUNT(*) FILTER (WHERE et.nombre = 'confirmado') AS confirmados, \
                COUNT(*) FILTER (WHERE et.nombre = 'completado') AS completados, \
                COUNT(*) FILTER (WHERE et.nombre = 'cancelado') AS cancelados, \
                COUNT(*) FILTER (WHERE et.nombre = 'no_asistio') AS no_asistio \
         FROM turno t JOIN estado_turno et ON et.id = t.estado_turno_id \
         WHERE t.empresa_id = $1 AND t.fecha BETWEEN $2 AND $3",
    )
    .bind(id)
    .bind(desde)
    .bind(hasta)
    .fetch_one(&state.db)
    .await?;
    let (calificaciones, calificacion_promedio) = sqlx::query_as::<_, (i64, Option<f64>)>(
        "SELECT COUNT(*), ROUND(AVG(puntaje)::numeric, 1)::float8 FROM calificacion \
         WHERE empresa_id = $1 AND creado_en::date BETWEEN $2 AND $3",
    )
    .bind(id)
    .bind(desde)
    .bind(hasta)
    .fetch_one(&state.db)
    .await?;
    let favoritos_nuevos = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM favorito WHERE empresa_id = $1 AND creado_en::date BETWEEN $2 AND $3",
    )
    .bind(id)
    .bind(desde)
    .bind(hasta)
    .fetch_one(&state.db)
    .await?;
    let por_dia = sqlx::query_as::<_, TurnosDelDia>(
        "SELECT t.fecha, COUNT(*) FILTER (WHERE et.nombre <> 'cancelado') AS turnos, \
                COUNT(*) FILTER (WHERE et.nombre = 'cancelado') AS cancelados \
         FROM turno t JOIN estado_turno et ON et.id = t.estado_turno_id \
         WHERE t.empresa_id = $1 AND t.fecha BETWEEN $2 AND $3 \
         GROUP BY t.fecha ORDER BY t.fecha",
    )
    .bind(id)
    .bind(desde)
    .bind(hasta)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(Resumen {
        empresa_id: id,
        empresa_nombre,
        desde,
        hasta,
        turnos,
        calificaciones,
        calificacion_promedio,
        favoritos_nuevos,
        por_dia,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dia: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dia).unwrap()
    }

    #[test]
    fn sin_fechas_toma_los_ultimos_30_dias() {
        let p = Periodo { desde: None, hasta: None };
        assert_eq!(periodo(&p, d(2026, 11, 30)).unwrap(), (d(2026, 11, 1), d(2026, 11, 30)));
    }

    #[test]
    fn rechaza_periodos_invertidos_o_de_mas_de_un_anio() {
        let invertido = Periodo { desde: Some(d(2026, 11, 10)), hasta: Some(d(2026, 11, 1)) };
        assert!(periodo(&invertido, d(2026, 11, 30)).is_err());
        let largo = Periodo { desde: Some(d(2024, 12, 31)), hasta: Some(d(2026, 1, 1)) };
        assert!(periodo(&largo, d(2026, 11, 30)).is_err());
        let un_mes = Periodo { desde: Some(d(2026, 10, 1)), hasta: Some(d(2026, 10, 31)) };
        assert!(periodo(&un_mes, d(2026, 11, 30)).is_ok());
    }
}
