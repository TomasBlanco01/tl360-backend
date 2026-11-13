use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
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
use crate::validation::{email_valido, nombre_valido, texto_hasta_opt};

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
        sqlx::query_scalar::<_, bool>("SELECT email_verificado FROM usuario WHERE id = $1")
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
/// Lo garantiza el índice único parcial `turno_slot_activo_uq` (turnos no cancelados) junto con
/// `ON CONFLICT DO NOTHING`; no depende de un chequeo previo, que dos requests podrían pisarse.
/// Antes de insertar se valida contra los horarios del comercio (día, franja, grilla, no pasado).
pub async fn reservar_turno(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<ReservarRequest>,
) -> AppResult<Json<Value>> {
    // La reserva online es del cliente: es quien la ve y la cancela en "Mi cuenta". El dueño
    // agenda en su comercio desde Mi Negocio y el superadmin no opera como cliente.
    if user.rol != "cliente" {
        return Err(AppError::sin_permisos_por("las reservas online se hacen con una cuenta de cliente; el comercio agenda sus turnos desde Mi Negocio"));
    }
    exigir_email_verificado(&state, user.id).await?;
    let hora = parse_hora(&body.hora)?;

    let (habilitado, duracion) = sqlx::query_as::<_, (bool, Option<i32>)>(
        "SELECT turnos_habilitados, duracion_turno_min::int4 FROM empresa WHERE id = $1",
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
        "SELECT hora_inicio, hora_fin FROM horario_empresa WHERE empresa_id = $1 AND dia_semana = $2",
    )
    .bind(body.empresa_id)
    .bind(nombre_dia(body.fecha))
    .fetch_all(&state.db)
    .await?;
    validar_slot(body.fecha, hora, duracion, &franjas, ahora_local())
        .map_err(AppError::BadRequest)?;

    let insertado = sqlx::query_scalar::<_, i32>(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, origen, estado_turno_id) \
         VALUES ($1, $2, $3, $4, 'online', 1) \
         ON CONFLICT (empresa_id, fecha, hora) WHERE estado_turno_id <> 2 DO NOTHING \
         RETURNING id",
    )
    .bind(body.empresa_id)
    .bind(user.id)
    .bind(body.fecha)
    .bind(hora)
    .fetch_optional(&state.db)
    .await?;

    let Some(id) = insertado else {
        return Err(AppError::Conflict("Turno ya ocupado".into()));
    };
    enviar_confirmacion(&state, &user, body.empresa_id, body.fecha, hora).await;
    // El id sirve al front para el evento de calendario (UID estable del .ics).
    Ok(Json(json!({ "message": "Turno reservado correctamente", "id": id })))
}

#[derive(Deserialize)]
pub struct CargarTurnoRequest {
    fecha: NaiveDate,
    hora: String,
    cliente_nombre: String,
    cliente_email: Option<String>,
    /// Nota libre del comercio (hasta 300 caracteres).
    nota: Option<String>,
}

/// RF-04: turno de mostrador. El comercio lo agenda para alguien sin cuenta (por teléfono o en el
/// local) y deja su nombre y, si los tiene, su correo y una nota; queda con origen 'mostrador'. Usa
/// la misma grilla y el mismo índice único que la reserva online, así que no puede pisar un horario
/// ya tomado. No exige reservas online habilitadas: el comercio puede agendar aunque no ofrezca
/// turnos por la web.
pub async fn cargar_turno(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(empresa_id): Path<i32>,
    AppJson(body): AppJson<CargarTurnoRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    crate::handlers::empresas::verificar_dueno(&state, &user, empresa_id).await?;
    let nombre = body.cliente_nombre.trim();
    nombre_valido(nombre)?;
    let email = body
        .cliente_email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    if let Some(e) = email {
        email_valido(e)?;
    }
    texto_hasta_opt("La nota", &body.nota, 300)?;
    let nota = body.nota.as_deref().map(str::trim).filter(|n| !n.is_empty());
    let hora = parse_hora(&body.hora)?;

    let (duracion, comercio, direccion) = sqlx::query_as::<_, (Option<i32>, String, String)>(
        "SELECT duracion_turno_min::int4, nombre, direccion FROM empresa WHERE id = $1",
    )
    .bind(empresa_id)
    .fetch_one(&state.db)
    .await?;
    let franjas = sqlx::query_as::<_, (NaiveTime, NaiveTime)>(
        "SELECT hora_inicio, hora_fin FROM horario_empresa WHERE empresa_id = $1 AND dia_semana = $2",
    )
    .bind(empresa_id)
    .bind(nombre_dia(body.fecha))
    .fetch_all(&state.db)
    .await?;
    validar_slot(body.fecha, hora, duracion, &franjas, ahora_local())
        .map_err(AppError::BadRequest)?;

    let id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO turno (empresa_id, fecha, hora, origen, estado_turno_id, cliente_nombre, cliente_email, nota) \
         VALUES ($1, $2, $3, 'mostrador', 1, $4, $5, $6) \
         ON CONFLICT (empresa_id, fecha, hora) WHERE estado_turno_id <> 2 DO NOTHING \
         RETURNING id",
    )
    .bind(empresa_id)
    .bind(body.fecha)
    .bind(hora)
    .bind(nombre)
    .bind(email)
    .bind(nota)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::Conflict("Turno ya ocupado".into()))?;

    if let Some(email) = email {
        let cuerpo = crate::mailer::cuerpo_turno_confirmado(
            nombre,
            &comercio,
            &body.fecha.format("%d/%m/%Y").to_string(),
            &hora.format("%H:%M").to_string(),
            &direccion,
        );
        state
            .mailer
            .enviar_o_loggear(email, "Tu turno en TL360 quedó confirmado", cuerpo)
            .await;
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "message": "Turno agendado correctamente" })),
    ))
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
        "SELECT e.nombre, e.direccion, u.email, e.usuario_id \
         FROM empresa e JOIN usuario u ON u.id = $2 WHERE e.id = $1",
    )
    .bind(empresa_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await;
    match datos {
        Ok(Some((comercio, direccion, email, dueno_id))) => {
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
            if let Some(dueno_id) = dueno_id {
                let cuerpo = format!(
                    "{} reservó para el {fecha} a las {hora} en {comercio}",
                    user.nombre
                );
                state
                    .push
                    .enviar_a_usuario(
                        &state.db,
                        dueno_id,
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
    /// Para el evento de calendario (.ics / Google Calendar) del turno.
    empresa_direccion: String,
    duracion_min: Option<i32>,
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
        SELECT t.id, t.fecha, t.hora, et.nombre AS estado, t.empresa_id, e.nombre AS empresa_nombre,
               e.slug AS empresa_slug, e.direccion AS empresa_direccion,
               e.duracion_turno_min::int4 AS duracion_min, (k.id IS NOT NULL) AS calificado
        FROM turno t
        JOIN estado_turno et ON et.id = t.estado_turno_id
        JOIN empresa e ON e.id = t.empresa_id
        LEFT JOIN calificacion k ON k.turno_id = t.id
        WHERE t.usuario_id = $1 AND ($2 OR et.nombre <> 'cancelado')
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
        "SELECT hora FROM turno WHERE empresa_id = $1 AND fecha = $2 AND estado_turno_id <> 2 ORDER BY hora",
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
    /// NULL en los turnos que el comercio cargó para alguien sin cuenta.
    cliente_id: Option<i32>,
    cliente_nombre: String,
    cliente_email: Option<String>,
    dueno_id: Option<i32>,
}

/// RF-05: cancela el cliente dueño del turno (con la anticipación mínima del comercio) o el dueño
/// del comercio (sin esa restricción y avisando al cliente). El turno no se borra: pasa a
/// `cancelado` (estado_turno) y su horario queda libre para otros clientes.
pub async fn cancelar_turno(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    let turno = sqlx::query_as::<_, TurnoCancelable>(
        "SELECT t.fecha, t.hora, e.anticipacion_cancelacion_min::int4 AS anticipacion, \
                e.nombre AS comercio, t.usuario_id AS cliente_id, \
                COALESCE(u.nombre, t.cliente_nombre) AS cliente_nombre, \
                COALESCE(u.email, t.cliente_email) AS cliente_email, e.usuario_id AS dueno_id \
         FROM turno t \
         JOIN empresa e ON e.id = t.empresa_id \
         LEFT JOIN usuario u ON u.id = t.usuario_id \
         WHERE t.id = $1 AND t.estado_turno_id = 1 \
           AND (t.usuario_id = $2 OR e.usuario_id = $2)",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound(
        "Turno no encontrado o no autorizado".into(),
    ))?;

    let es_dueno = turno.dueno_id == Some(user.id);
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
        "UPDATE turno t SET estado_turno_id = (SELECT id FROM estado_turno WHERE nombre = 'cancelado') \
         FROM empresa e \
         WHERE t.id = $1 AND e.id = t.empresa_id \
           AND t.estado_turno_id = (SELECT id FROM estado_turno WHERE nombre = 'confirmado') \
           AND (t.usuario_id = $2 OR e.usuario_id = $2)",
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
        // Un turno cargado sin cuenta solo se avisa por correo, y solo si la persona dejó uno.
        if let Some(email) = &turno.cliente_email {
            let cuerpo = crate::mailer::cuerpo_turno_cancelado_por_comercio(
                &turno.cliente_nombre,
                &turno.comercio,
                &fecha,
                &hora,
            );
            state
                .mailer
                .enviar_o_loggear(email, "Tu turno en TL360 fue cancelado", cuerpo)
                .await;
        }
        if let Some(cliente_id) = turno.cliente_id {
            let cuerpo = format!(
                "{} canceló tu turno del {fecha} a las {hora} hs. El horario quedó libre.",
                turno.comercio
            );
            state
                .push
                .enviar_a_usuario(
                    &state.db,
                    cliente_id,
                    &crate::push::Notificacion {
                        titulo: "El comercio canceló tu turno",
                        cuerpo: &cuerpo,
                        url: "/cuenta",
                    },
                )
                .await;
        }
    } else {
        let cuerpo = crate::mailer::cuerpo_turno_cancelado(
            &turno.cliente_nombre,
            &turno.comercio,
            &fecha,
            &hora,
        );
        if let Some(email) = &turno.cliente_email {
            state
                .mailer
                .enviar_o_loggear(email, "Tu turno en TL360 fue cancelado", cuerpo)
                .await;
        }
        if let Some(dueno_id) = turno.dueno_id {
            let cuerpo = format!(
                "{} canceló el turno del {fecha} a las {hora} en {}",
                turno.cliente_nombre, turno.comercio
            );
            state
                .push
                .enviar_a_usuario(
                    &state.db,
                    dueno_id,
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
