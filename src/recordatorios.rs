//! RF-03: recordatorio push de turnos. Un proceso liviano revisa cada 15 minutos los turnos
//! confirmados dentro de la anticipación elegida por cada cliente, avisa a quienes tienen push y
//! marca el turno para no repetir el aviso. Solo corre si Web Push está configurado. Sin
//! preferencia configurada se usan 24 h.

use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use sqlx::{FromRow, PgPool};

use crate::agenda::ahora_local;
use crate::push::Notificacion;
use crate::state::AppState;

#[derive(FromRow)]
pub struct Recordatorio {
    pub turno_id: i32,
    pub usuario_id: i32,
    pub empresa_nombre: String,
    pub fecha: NaiveDate,
    pub hora: NaiveTime,
}

pub fn bucle(state: AppState) {
    tokio::spawn(async move {
        let mut intervalo = tokio::time::interval(Duration::from_secs(15 * 60));
        loop {
            intervalo.tick().await;
            let enviados = enviar_pendientes(&state).await;
            if enviados > 0 {
                tracing::info!("recordatorios push enviados: {enviados}");
            }
        }
    });
}

async fn enviar_pendientes(state: &AppState) -> usize {
    let desde = ahora_local();
    let turnos = match pendientes(&state.db, desde).await {
        Ok(turnos) => turnos,
        Err(e) => {
            tracing::warn!("no se pudieron buscar turnos para recordar: {e}");
            return 0;
        }
    };
    let mut enviados = 0;
    for turno in turnos {
        let cuerpo = format!(
            "{} a las {} en {}",
            si_es_hoy(turno.fecha, desde.date()),
            turno.hora.format("%H:%M"),
            turno.empresa_nombre
        );
        let notificacion = Notificacion {
            titulo: "Recordatorio de turno",
            cuerpo: &cuerpo,
            url: "/cuenta",
        };
        state
            .push
            .enviar_a_usuario(&state.db, turno.usuario_id, &notificacion)
            .await;
        if let Err(e) = marcar_enviado(&state.db, turno.turno_id).await {
            tracing::warn!(
                "no se pudo marcar el recordatorio del turno {}: {e}",
                turno.turno_id
            );
        }
        enviados += 1;
    }
    enviados
}

fn si_es_hoy(fecha: NaiveDate, hoy: NaiveDate) -> &'static str {
    if fecha == hoy {
        "Hoy"
    } else {
        "Mañana"
    }
}

/// Turnos confirmados que ya entraron en la ventana de anticipación de cada cliente y cuyos
/// dueños tienen al menos un dispositivo suscripto.
pub async fn pendientes(
    db: &PgPool,
    desde: NaiveDateTime,
) -> Result<Vec<Recordatorio>, sqlx::Error> {
    sqlx::query_as::<_, Recordatorio>(
        "SELECT t.id AS turno_id, t.usuario_id, e.nombre AS empresa_nombre, t.fecha, t.hora \
         FROM turnos t \
         JOIN empresas e ON e.id = t.empresa_id \
         JOIN usuarios u ON u.id = t.usuario_id \
         WHERE t.estado = 'confirmado' AND t.recordatorio_enviado_en IS NULL \
           AND (t.fecha + t.hora) > $1 \
           AND (t.fecha + t.hora) <= $1 + make_interval(hours => u.recordatorio_horas_anticipacion) \
           AND EXISTS (SELECT 1 FROM push_suscripciones p WHERE p.usuario_id = t.usuario_id) \
         ORDER BY t.fecha, t.hora LIMIT 200",
    )
    .bind(desde)
    .fetch_all(db)
    .await
}

pub async fn marcar_enviado(db: &PgPool, turno_id: i32) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE turnos SET recordatorio_enviado_en = now() WHERE id = $1")
        .bind(turno_id)
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_texto_del_recordatorio_distingue_hoy_de_manana() {
        let hoy = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        assert_eq!(si_es_hoy(hoy, hoy), "Hoy");
        assert_eq!(si_es_hoy(hoy + chrono::Duration::days(1), hoy), "Mañana");
    }
}
