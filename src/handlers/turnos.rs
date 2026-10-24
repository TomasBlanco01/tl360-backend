use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::agenda::{ahora_local, nombre_dia, puede_cancelar, texto_anticipacion, validar_slot};
use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ReservarRequest {
    empresa_id: i32,
    fecha: NaiveDate,
    hora: String,
}

/// RF-01: reservar y cancelar turnos como cliente exige cuenta verificada. Las cuentas creadas por
/// registro nacen sin verificar; las cargadas directo en la base o por un superadmin ya
/// vienen verificadas (o se verifican con el enlace). El dueño del comercio no necesita el email
/// verificado para cancelar turnos de su propio local (RF-05).
async fn exigir_email_verificado(state: &AppState, user_id: i32) -> AppResult<()> {
    let verificado =
        sqlx::query_scalar::<_, bool>("SELECT email_verificado FROM usuarios WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?
            .unwrap_or(false);
    if !verificado {
        return Err(AppError::Forbidden(
            "Tenés que verificar tu correo electrónico antes de gestionar turnos. Revisá tu correo o pedí un nuevo enlace.".into(),
        ));
    }
    Ok(())
}

/// El frontend manda "HH:MM" (sin segundos); se acepta también "HH:MM:SS" por si acaso.
fn parse_hora(s: &str) -> AppResult<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M:%S"))
        .map_err(|_| AppError::BadRequest("Hora inválida, se espera HH:MM".into()))
}

/// RF-03 v2: "ante dos solicitudes simultáneas para el mismo horario, únicamente la primera en
/// confirmarse será aceptada; la segunda recibirá un aviso de que el horario ya no está disponible".
/// Lo garantiza el índice único parcial `turnos_slot_activo_uq` (migración 0002) junto con
/// `ON CONFLICT DO NOTHING`; no depende de un chequeo previo, que dos requests podrían pisarse.
/// Antes de insertar se valida contra los horarios del comercio (día, franja, grilla, no pasado).
pub async fn reservar_turno(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<ReservarRequest>,
) -> AppResult<Json<Value>> {
    exigir_email_verificado(&state, user.id).await?;
    let hora = parse_hora(&body.hora)?;

    let (habilitado, duracion) = sqlx::query_as::<_, (bool, Option<i32>)>(
        "SELECT turnos_habilitados, duracion_turno_min FROM empresas WHERE id = $1",
    )
    .bind(body.empresa_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("Empresa no encontrada".into()))?;
    if !habilitado {
        return Err(AppError::BadRequest(
            "Este comercio no toma turnos online".into(),
        ));
    }

    let franjas = sqlx::query_as::<_, (NaiveTime, NaiveTime)>(
        "SELECT hora_inicio, hora_fin FROM horarios_empresa WHERE empresa_id = $1 AND dia_semana = $2",
    )
    .bind(body.empresa_id)
    .bind(nombre_dia(body.fecha))
    .fetch_all(&state.db)
    .await?;
    validar_slot(body.fecha, hora, duracion, &franjas, ahora_local())
        .map_err(AppError::BadRequest)?;

    let insertado = sqlx::query_scalar::<_, i32>(
        "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
         VALUES ($1, $2, $3, $4, 'confirmado') \
         ON CONFLICT (empresa_id, fecha, hora) WHERE estado <> 'cancelado' DO NOTHING \
         RETURNING id",
    )
    .bind(body.empresa_id)
    .bind(user.id)
    .bind(body.fecha)
    .bind(hora)
    .fetch_optional(&state.db)
    .await?;

    if insertado.is_none() {
        return Err(AppError::Conflict("Turno ya ocupado".into()));
    }
    enviar_confirmacion(&state, &user, body.empresa_id, body.fecha, hora).await;
    Ok(Json(json!({ "message": "Turno reservado correctamente" })))
}

/// RF-03: aviso de confirmación por correo. Un fallo de correo no revierte la reserva.
async fn enviar_confirmacion(
    state: &AppState,
    user: &CurrentUser,
    empresa_id: i32,
    fecha: NaiveDate,
    hora: NaiveTime,
) {
    let datos = sqlx::query_as::<_, (String, String, String, Option<i32>)>(
        "SELECT e.nombre, e.direccion, u.email, e.admin_id \
         FROM empresas e JOIN usuarios u ON u.id = $2 WHERE e.id = $1",
    )
    .bind(empresa_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await;
    match datos {
        Ok(Some((comercio, direccion, email, admin_id))) => {
            let fecha = fecha.format("%d/%m/%Y").to_string();
            let hora = hora.format("%H:%M").to_string();
            let cuerpo = crate::mailer::cuerpo_turno_confirmado(
                &user.nombre,
                &comercio,
                &fecha,
                &hora,
                &direccion,
            );
            state
                .mailer
                .enviar_o_loggear(&email, "Tu turno en TL360 quedó confirmado", cuerpo)
                .await;
            if let Some(admin_id) = admin_id {
                let cuerpo = format!(
                    "{} reservó para el {fecha} a las {hora} en {comercio}",
                    user.nombre
                );
                state
                    .push
                    .enviar_a_usuario(
                        &state.db,
                        admin_id,
                        &crate::push::Notificacion {
                            titulo: "Nuevo turno reservado",
                            cuerpo: &cuerpo,
                            url: "/negocio",
                        },
                    )
                    .await;
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("no se pudo preparar el aviso de la reserva: {e}"),
    }
}

#[derive(FromRow, Serialize)]
pub struct MiTurno {
    id: i32,
    fecha: NaiveDate,
    hora: NaiveTime,
    estado: String,
    empresa_id: i32,
    empresa_nombre: String,
    empresa_slug: Option<String>,
    calificado: bool,
}

#[derive(Deserialize)]
pub struct MisTurnosQuery {
    incluir_cancelados: Option<bool>,
}

/// RF-09: historial del cliente. Por defecto no muestra los cancelados (el front no distingue estados);
/// `?incluir_cancelados=true` los agrega.
pub async fn get_mis_turnos(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(q): Query<MisTurnosQuery>,
) -> AppResult<Json<Vec<MiTurno>>> {
    let rows = sqlx::query_as::<_, MiTurno>(
        r#"
        SELECT t.id, t.fecha, t.hora, t.estado, t.empresa_id, e.nombre AS empresa_nombre,
               e.slug AS empresa_slug, (k.id IS NOT NULL) AS calificado
        FROM turnos t
        JOIN empresas e ON e.id = t.empresa_id
        LEFT JOIN calificaciones k ON k.turno_id = t.id
        WHERE t.usuario_id = $1 AND ($2 OR t.estado <> 'cancelado')
        ORDER BY t.fecha ASC, t.hora ASC
        "#,
    )
    .bind(user.id)
    .bind(q.incluir_cancelados.unwrap_or(false))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct OcupadosQuery {
    empresa_id: i32,
    fecha: NaiveDate,
}

#[derive(Serialize)]
pub struct HoraOcupada {
    hora: String,
}

pub async fn get_turnos_ocupados(
    State(state): State<AppState>,
    Query(q): Query<OcupadosQuery>,
) -> AppResult<Json<Vec<HoraOcupada>>> {
    let horas = sqlx::query_scalar::<_, NaiveTime>(
        "SELECT hora FROM turnos WHERE empresa_id = $1 AND fecha = $2 AND estado <> 'cancelado' ORDER BY hora",
    )
    .bind(q.empresa_id)
    .bind(q.fecha)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        horas
            .into_iter()
            .map(|h| HoraOcupada {
                hora: h.format("%H:%M").to_string(),
            })
            .collect(),
    ))
}

#[derive(FromRow)]
struct TurnoCancelable {
    fecha: NaiveDate,
    hora: NaiveTime,
    anticipacion: i32,
    comercio: String,
    cliente_id: i32,
    cliente_nombre: String,
    cliente_email: String,
    admin_id: Option<i32>,
}

/// RF-05: cancela el cliente dueño del turno (con la anticipación mínima del comercio) o el dueño
/// del comercio (sin esa restricción y avisando al cliente). El turno no se borra: pasa a
/// `cancelado` y su horario queda libre para otros clientes.
pub async fn cancelar_turno(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    let turno = sqlx::query_as::<_, TurnoCancelable>(
        "SELECT t.fecha, t.hora, e.anticipacion_cancelacion_min AS anticipacion, \
                e.nombre AS comercio, t.usuario_id AS cliente_id, u.nombre AS cliente_nombre, \
                u.email AS cliente_email, e.admin_id \
         FROM turnos t \
         JOIN empresas e ON e.id = t.empresa_id \
         JOIN usuarios u ON u.id = t.usuario_id \
         WHERE t.id = $1 AND t.estado = 'confirmado' \
           AND (t.usuario_id = $2 OR e.admin_id = $2)",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound(
        "Turno no encontrado o no autorizado".into(),
    ))?;

    let es_dueno = turno.admin_id == Some(user.id);
    let ahora = ahora_local();
    if es_dueno {
        if turno.fecha.and_time(turno.hora) <= ahora {
            return Err(AppError::BadRequest(
                "Solo se pueden cancelar turnos futuros".into(),
            ));
        }
    } else {
        exigir_email_verificado(&state, user.id).await?;
        if !puede_cancelar(ahora, turno.fecha, turno.hora, turno.anticipacion) {
            return Err(AppError::BadRequest(format!(
                "Solo se puede cancelar un turno con al menos {} de anticipación",
                texto_anticipacion(turno.anticipacion)
            )));
        }
    }

    let result = sqlx::query(
        "UPDATE turnos t SET estado = 'cancelado' \
         FROM empresas e \
         WHERE t.id = $1 AND e.id = t.empresa_id AND t.estado = 'confirmado' \
           AND (t.usuario_id = $2 OR e.admin_id = $2)",
    )
    .bind(id)
    .bind(user.id)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(
            "Turno no encontrado o no autorizado".into(),
        ));
    }

    let fecha = turno.fecha.format("%d/%m/%Y").to_string();
    let hora = turno.hora.format("%H:%M").to_string();
    if es_dueno {
        let cuerpo = crate::mailer::cuerpo_turno_cancelado_por_comercio(
            &turno.cliente_nombre,
            &turno.comercio,
            &fecha,
            &hora,
        );
        state
            .mailer
            .enviar_o_loggear(
                &turno.cliente_email,
                "Tu turno en TL360 fue cancelado",
                cuerpo,
            )
            .await;
        let cuerpo = format!(
            "{} canceló tu turno del {fecha} a las {hora} hs. El horario quedó libre.",
            turno.comercio
        );
        state
            .push
            .enviar_a_usuario(
                &state.db,
                turno.cliente_id,
                &crate::push::Notificacion {
                    titulo: "El comercio canceló tu turno",
                    cuerpo: &cuerpo,
                    url: "/cuenta",
                },
            )
            .await;
    } else {
        let cuerpo = crate::mailer::cuerpo_turno_cancelado(
            &turno.cliente_nombre,
            &turno.comercio,
            &fecha,
            &hora,
        );
        state
            .mailer
            .enviar_o_loggear(
                &turno.cliente_email,
                "Tu turno en TL360 fue cancelado",
                cuerpo,
            )
            .await;
        if let Some(admin_id) = turno.admin_id {
            let cuerpo = format!(
                "{} canceló el turno del {fecha} a las {hora} en {}",
                turno.cliente_nombre, turno.comercio
            );
            state
                .push
                .enviar_a_usuario(
                    &state.db,
                    admin_id,
                    &crate::push::Notificacion {
                        titulo: "Turno cancelado",
                        cuerpo: &cuerpo,
                        url: "/negocio",
                    },
                )
                .await;
        }
    }
    Ok(Json(json!({ "message": "Turno cancelado correctamente" })))
}
