use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use std::collections::{HashMap, HashSet};

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::state::AppState;
use crate::validation::{url_http, url_http_opt};

#[derive(FromRow, Serialize)]
pub struct Empresa {
    id: i32,
    nombre: String,
    direccion: String,
    telefono: Option<String>,
    slogan: Option<String>,
    duracion_turno_min: Option<i32>,
    categoria_id: i32,
    admin_id: Option<i32>,
    instagram: Option<String>,
    facebook: Option<String>,
    fotos: Vec<String>,
    turnos_habilitados: bool,
    verificado: bool,
    verificado_estado: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cuit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    constancia_cuit_url: Option<String>,
    lat: Option<f64>,
    lng: Option<f64>,
    modo_contacto: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    descripcion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    categoria: Option<String>,
    /// RF-18: etiquetas descriptivas del comercio, ordenadas alfabéticamente.
    etiquetas: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    calificacion_promedio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    calificacion_cantidad: Option<i64>,
    /// RF-07: distancia al punto buscado, solo cuando se envía `?lat=&lng=`.
    #[serde(skip_serializing_if = "Option::is_none")]
    distancia_km: Option<f64>,
    /// RF-07: si el comercio está en su franja horaria ahora, para ordenar abiertos primero.
    #[serde(skip_serializing_if = "Option::is_none")]
    abierto_ahora: Option<bool>,
}

#[derive(FromRow, Serialize)]
pub struct Telefono {
    id: i32,
    numero: String,
    etiqueta: Option<String>,
    modo_contacto: Option<String>,
}

/// SELECT común de lectura. En el listado se omite la descripción (texto largo × miles de comercios)
/// y se devuelve completa solo en la ficha individual.
/// Distancia haversine en km para el punto `$8`/`$9`; NULL si el comercio no tiene coordenadas.
const DISTANCIA_KM: &str = "CASE WHEN e.lat IS NULL OR e.lng IS NULL THEN NULL ELSE \
    6371.0 * 2.0 * asin(LEAST(1.0, sqrt( \
      power(sin(radians(e.lat::float8 - $8::float8) / 2.0), 2) + \
      cos(radians($8::float8)) * cos(radians(e.lat::float8)) * \
      power(sin(radians(e.lng::float8 - $9::float8) / 2.0), 2) \
    ))) END";

/// RF-07: usa el día/hora local (`$10`/`$11`) para saber si el comercio está abierto.
const ABIERTO_AHORA: &str = "EXISTS (SELECT 1 FROM horarios_empresa h \
    WHERE h.empresa_id = e.id AND h.dia_semana = $10::text \
      AND $11::time >= h.hora_inicio AND $11::time < h.hora_fin)";

fn read_select(con_descripcion: bool, con_distancia: bool) -> String {
    let desc = if con_descripcion {
        "e.descripcion"
    } else {
        "NULL::text"
    };
    let (distancia, abierto) = if con_distancia {
        (
            format!("{DISTANCIA_KM} AS distancia_km"),
            format!("{ABIERTO_AHORA} AS abierto_ahora"),
        )
    } else {
        (
            "NULL::float8 AS distancia_km".to_string(),
            "NULL::bool AS abierto_ahora".to_string(),
        )
    };
    format!(
        "SELECT e.id, e.nombre, e.direccion, e.telefono, e.slogan, e.duracion_turno_min, e.categoria_id, \
         e.admin_id, e.instagram, e.facebook, e.fotos, e.turnos_habilitados, e.verificado, e.verificado_estado, \
         e.cuit, e.constancia_cuit_url, e.lat::float8 AS lat, e.lng::float8 AS lng, \
         e.modo_contacto, {desc} AS descripcion, c.nombre AS categoria, \
         COALESCE((SELECT array_agg(et.nombre ORDER BY et.nombre) \
                   FROM empresa_etiquetas ee JOIN etiquetas et ON et.id = ee.etiqueta_id \
                   WHERE ee.empresa_id = e.id), ARRAY[]::text[]) AS etiquetas, \
         (SELECT ROUND(AVG(k.puntaje)::numeric, 1)::float8 FROM calificaciones k WHERE k.empresa_id = e.id) AS calificacion_promedio, \
         (SELECT COUNT(*) FROM calificaciones k WHERE k.empresa_id = e.id) AS calificacion_cantidad, \
         {distancia}, {abierto} \
         FROM empresas e LEFT JOIN categorias c ON c.id = e.categoria_id"
    )
}

async fn fetch_empresa(db: &PgPool, id: i32) -> AppResult<Empresa> {
    sqlx::query_as::<_, Empresa>(&format!("{} WHERE e.id = $1", read_select(true, false)))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound("Empresa no encontrada".into()))
}

#[derive(Deserialize)]
pub struct ListQuery {
    q: Option<String>,
    categoria_id: Option<i32>,
    admin_id: Option<i32>,
    verificado_estado: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    /// RF-07: punto de referencia para ordenar por cercanía. Se envían juntos.
    lat: Option<f64>,
    lng: Option<f64>,
    /// RF-18: etiquetas separadas por coma; se exigen todas (AND).
    etiquetas: Option<String>,
}

/// Catálogo canónico de RF-18 y sus alias habituales (claves sin acentos ni signos).
const CATALOGO_ETIQUETAS: [(&str, &[&str]); 5] = [
    (
        "Rampa de acceso",
        &[
            "rampa",
            "rampadeacceso",
            "accesoparasillasderuedas",
            "sillasderuedas",
        ],
    ),
    (
        "Baño adaptado",
        &["bano", "banio", "banoadaptado", "banioadaptado"],
    ),
    (
        "Apto celíacos",
        &["aptoceliaco", "aptoceliacos", "celiaco", "celiacos"],
    ),
    (
        "Acepta mascotas",
        &[
            "mascota",
            "mascotas",
            "aceptamascota",
            "aceptamascotas",
            "petfriendly",
        ],
    ),
    (
        "Wi-Fi gratis",
        &["wifi", "wifigratis", "internet", "internetgratis"],
    ),
];

const MAX_ETIQUETAS: usize = 12;
const MAX_LARGO_ETIQUETA: usize = 40;

/// Clave de comparación: minúsculas, sin acentos ni signos.
fn clave_etiqueta(valor: &str) -> String {
    valor
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            _ => c,
        })
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Resuelve alias conocidos al nombre canónico; deja intactas las etiquetas libres.
fn etiqueta_canonica(valor: &str) -> String {
    let clave = clave_etiqueta(valor);
    CATALOGO_ETIQUETAS
        .iter()
        .find(|(_, alias)| alias.contains(&clave.as_str()))
        .map_or_else(
            || valor.trim().to_string(),
            |(nombre, _)| (*nombre).to_string(),
        )
}

fn etiquetas_filtro(raw: &Option<String>) -> Option<Vec<String>> {
    let mut vistas = HashSet::new();
    let mut salida = Vec::new();
    for pedida in raw.as_deref().unwrap_or_default().split(',') {
        if pedida.trim().is_empty() {
            continue;
        }
        let nombre = etiqueta_canonica(pedida);
        if !nombre.is_empty() && vistas.insert(clave_etiqueta(&nombre)) {
            salida.push(nombre);
        }
    }
    (!salida.is_empty()).then_some(salida)
}

/// `None` conserva las etiquetas actuales; `Some([])` las borra.
fn normalizar_etiquetas(valor: &Option<Vec<String>>) -> AppResult<Option<Vec<String>>> {
    let Some(lista) = valor else {
        return Ok(None);
    };
    if lista.len() > MAX_ETIQUETAS {
        return Err(AppError::BadRequest(format!(
            "Se permiten hasta {MAX_ETIQUETAS} etiquetas por comercio"
        )));
    }
    let mut vistas = HashSet::new();
    let mut salida = Vec::new();
    for etiqueta in lista {
        let nombre = etiqueta_canonica(etiqueta);
        if nombre.is_empty() {
            continue;
        }
        if nombre.chars().count() > MAX_LARGO_ETIQUETA {
            return Err(AppError::BadRequest(format!(
                "Cada etiqueta puede tener hasta {MAX_LARGO_ETIQUETA} caracteres"
            )));
        }
        if vistas.insert(clave_etiqueta(&nombre)) {
            salida.push(nombre);
        }
    }
    Ok(Some(salida))
}

/// Sin `limit` se devuelve el catálogo completo (hasta este tope) por compatibilidad con el front actual.
const LIMITE_MAXIMO: i64 = 5000;

const ORDEN_LISTADO: &str = "ORDER BY e.verificado DESC, e.turnos_habilitados DESC, e.id";

/// RF-07: primero los geolocalizados, luego abiertos, y dentro de cada grupo los más cercanos.
const ORDEN_CERCANIA: &str = "ORDER BY (e.lat IS NULL OR e.lng IS NULL), abierto_ahora DESC, \
    distancia_km ASC, e.verificado DESC, e.turnos_habilitados DESC, e.id";

fn coordenadas(f: &ListQuery) -> AppResult<Option<(f64, f64)>> {
    match (f.lat, f.lng) {
        (None, None) => Ok(None),
        (Some(lat), Some(lng))
            if lat.is_finite()
                && lng.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lng) =>
        {
            Ok(Some((lat, lng)))
        }
        (Some(_), Some(_)) => Err(AppError::BadRequest(
            "Las coordenadas están fuera de rango".into(),
        )),
        _ => Err(AppError::BadRequest(
            "Enviá las dos coordenadas (lat y lng) juntas para ordenar por cercanía".into(),
        )),
    }
}

/// Minúsculas, sin tildes y sin puntuación: la misma forma que usa el diccionario.
fn normalizar_busqueda(texto: &str) -> String {
    let mut salida = String::with_capacity(texto.len());
    for c in texto.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            otro => otro,
        };
        salida.push(if c.is_alphanumeric() || c == ' ' {
            c
        } else {
            ' '
        });
    }
    salida.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// RF-07: reglas locales de lenguaje natural; no requiere un modelo de IA.
fn sinonimos_busqueda(palabra: &str) -> &'static [&'static str] {
    match palabra {
        "comida" => &["gastronomia", "restaurante", "bar", "comer", "menu"],
        "gastronomia" => &["gastronomia", "restaurante", "comida"],
        "restaurante" => &["gastronomia", "restaurante", "comida", "bar"],
        "peluqueria" => &[
            "peluqueria",
            "barberia",
            "corte",
            "pelo",
            "peinado",
            "color",
        ],
        "barberia" => &["barberia", "peluqueria", "corte", "barba", "fade"],
        "corte" => &["peluqueria", "barberia", "corte", "pelo"],
        "pelo" => &["peluqueria", "barberia", "pelo", "color"],
        "barba" => &["barberia", "barba", "fade"],
        "fade" => &["barberia", "fade", "corte"],
        "gym" | "gimnasio" => &["gimnasio", "fitness", "entrenamiento", "gym"],
        "pesas" => &["gimnasio", "fitness", "entrenamiento", "musculacion"],
        "fitness" => &["gimnasio", "fitness", "entrenamiento"],
        "crossfit" => &["gimnasio", "crossfit", "funcional"],
        "funcional" => &["gimnasio", "funcional", "entrenamiento"],
        "unas" | "manicuria" => &["manicuria", "pedicura", "unas", "nails"],
        "masaje" | "masajes" => &["masaje", "masajes", "relax", "spa"],
        "dentista" => &["odontologia", "dental"],
        "medico" => &["salud", "clinica", "consultorio"],
        "perro" | "gato" => &["veterinaria", "mascota"],
        "kiosco" | "almacen" => &["almacen", "kiosco", "drugstore"],
        "ropa" | "indumentaria" => &["indumentaria", "ropa", "calzado", "tienda"],
        "farmacia" => &["farmacia", "medicamento"],
        "yoga" | "pilates" => &["yoga", "pilates", "meditacion"],
        "tatuaje" | "tattoo" => &["tatuaje", "tattoo", "tinta"],
        _ => &[],
    }
}

fn expandir_busqueda(q: &str) -> Option<Vec<String>> {
    let base = normalizar_busqueda(q);
    if base.is_empty() {
        return None;
    }
    let mut terminos: Vec<String> = Vec::new();
    let mut agregar = |valor: &str| {
        let valor = normalizar_busqueda(valor);
        if !valor.is_empty() && !terminos.contains(&valor) {
            terminos.push(valor);
        }
    };
    agregar(&base);
    for palabra in base.split(' ') {
        agregar(palabra);
        for sinonimo in sinonimos_busqueda(palabra) {
            agregar(sinonimo);
        }
    }
    for sinonimo in sinonimos_busqueda(&base) {
        agregar(sinonimo);
    }
    Some(terminos)
}

pub async fn get_empresas(
    State(state): State<AppState>,
    Query(f): Query<ListQuery>,
) -> AppResult<Json<Vec<Empresa>>> {
    let geo = coordenadas(&f)?;
    let etiquetas = etiquetas_filtro(&f.etiquetas);
    let busqueda = f.q.as_deref().and_then(expandir_busqueda);
    let orden = if geo.is_some() {
        ORDEN_CERCANIA
    } else {
        ORDEN_LISTADO
    };
    let sql = format!(
        "{} WHERE ($1::text[] IS NULL OR EXISTS ( \
             SELECT 1 FROM unnest($1::text[]) AS buscado(termino) \
             WHERE translate(lower(e.nombre), 'áéíóúüñ', 'aeiouun') LIKE '%' || buscado.termino || '%' \
                OR translate(lower(COALESCE(c.nombre, '')), 'áéíóúüñ', 'aeiouun') LIKE '%' || buscado.termino || '%' \
                OR translate(lower(COALESCE(e.descripcion, '')), 'áéíóúüñ', 'aeiouun') LIKE '%' || buscado.termino || '%' \
                OR EXISTS ( \
                    SELECT 1 FROM empresa_etiquetas ee JOIN etiquetas et ON et.id = ee.etiqueta_id \
                    WHERE ee.empresa_id = e.id \
                      AND translate(lower(et.nombre), 'áéíóúüñ', 'aeiouun') LIKE '%' || buscado.termino || '%' \
                ) \
         )) \
         AND ($2::int4 IS NULL OR e.categoria_id = $2 \
              OR EXISTS (SELECT 1 FROM empresa_categorias ec WHERE ec.empresa_id = e.id AND ec.categoria_id = $2)) \
         AND ($5::int4 IS NULL OR e.admin_id = $5) \
         AND ($6::text IS NULL OR e.verificado_estado = $6) \
         AND ($7::text[] IS NULL OR NOT EXISTS ( \
             SELECT 1 FROM unnest($7::text[]) AS pedida(nombre) \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM empresa_etiquetas ee JOIN etiquetas et ON et.id = ee.etiqueta_id \
                 WHERE ee.empresa_id = e.id AND lower(et.nombre) = lower(pedida.nombre) \
             ) \
         )) \n         {orden} LIMIT $3::int8 OFFSET $4::int8",
        read_select(false, geo.is_some())
    );
    let mut consulta = sqlx::query_as::<_, Empresa>(&sql)
        .bind(busqueda)
        .bind(f.categoria_id)
        .bind(f.limit.map_or(LIMITE_MAXIMO, |l| l.clamp(1, 1000)))
        .bind(f.offset.unwrap_or(0).max(0))
        .bind(f.admin_id)
        .bind(
            f.verificado_estado
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .bind(etiquetas);
    if let Some((lat, lng)) = geo {
        let ahora = crate::agenda::ahora_local();
        consulta = consulta
            .bind(lat)
            .bind(lng)
            .bind(crate::agenda::nombre_dia(ahora.date()))
            .bind(ahora.time());
    }
    let rows = consulta.fetch_all(&state.db).await?;
    Ok(Json(rows))
}

pub async fn get_empresas_by_categoria(
    State(state): State<AppState>,
    Path(categoria_id): Path<i32>,
) -> AppResult<Json<Vec<Empresa>>> {
    let rows = sqlx::query_as::<_, Empresa>(&format!(
        "{} WHERE e.categoria_id = $1 \
         OR EXISTS (SELECT 1 FROM empresa_categorias ec WHERE ec.empresa_id = e.id AND ec.categoria_id = $1) \
         {ORDEN_LISTADO}",
        read_select(false, false)
    ))
    .bind(categoria_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

pub async fn get_empresa_by_id(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    let empresa = fetch_empresa(&state.db, id).await?;

    let telefonos = sqlx::query_as::<_, Telefono>(
        "SELECT id, numero, etiqueta, modo_contacto FROM telefonos_empresa WHERE empresa_id = $1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let mut value = serde_json::to_value(empresa).unwrap();
    value["telefonos"] = serde_json::to_value(telefonos).unwrap();
    Ok(Json(value))
}

pub async fn get_telefonos_by_empresa(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Vec<Telefono>>> {
    let rows = sqlx::query_as::<_, Telefono>(
        "SELECT id, numero, etiqueta, modo_contacto FROM telefonos_empresa WHERE empresa_id = $1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

const DIAS: [&str; 7] = [
    "Lunes",
    "Martes",
    "Miércoles",
    "Jueves",
    "Viernes",
    "Sábado",
    "Domingo",
];

#[derive(FromRow)]
struct HorarioRow {
    dia_semana: String,
    hora_inicio: NaiveTime,
    hora_fin: NaiveTime,
}

async fn horarios_de(db: &PgPool, empresa_id: i32) -> AppResult<HashMap<String, Vec<String>>> {
    let rows = sqlx::query_as::<_, HorarioRow>(
        "SELECT dia_semana, hora_inicio, hora_fin FROM horarios_empresa WHERE empresa_id = $1 ORDER BY hora_inicio",
    )
    .bind(empresa_id)
    .fetch_all(db)
    .await?;

    let mut horarios: HashMap<String, Vec<String>> = HashMap::new();
    for h in rows {
        // Formato "HH:MM", igual al que usa el prototipo mock (ver mock/data.ts).
        let rango = format!(
            "{}-{}",
            h.hora_inicio.format("%H:%M"),
            h.hora_fin.format("%H:%M")
        );
        let dia = horarios.entry(h.dia_semana.trim().to_string()).or_default();
        if !dia.contains(&rango) {
            dia.push(rango);
        }
    }
    Ok(horarios)
}

pub async fn get_horarios_by_empresa(
    State(state): State<AppState>,
    Path(empresa_id): Path<i32>,
) -> AppResult<Json<HashMap<String, Vec<String>>>> {
    Ok(Json(horarios_de(&state.db, empresa_id).await?))
}

#[derive(Deserialize)]
pub struct HorariosRequest {
    horarios: HashMap<String, Vec<String>>,
}

fn parse_rango(rango: &str) -> AppResult<(NaiveTime, NaiveTime)> {
    let malo = || {
        AppError::BadRequest(format!(
            "Franja horaria inválida: '{rango}' (se espera HH:MM-HH:MM)"
        ))
    };
    let (a, b) = rango.split_once('-').ok_or_else(malo)?;
    let ini = NaiveTime::parse_from_str(a.trim(), "%H:%M").map_err(|_| malo())?;
    let fin = NaiveTime::parse_from_str(b.trim(), "%H:%M").map_err(|_| malo())?;
    if fin <= ini {
        return Err(AppError::BadRequest(format!(
            "La franja '{rango}' termina antes de empezar"
        )));
    }
    Ok((ini, fin))
}

pub async fn update_horarios_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<HorariosRequest>,
) -> AppResult<Json<HashMap<String, Vec<String>>>> {
    // Se valida todo antes de tocar la base, para no dejar horarios a medio reemplazar.
    let mut filas: Vec<(&str, NaiveTime, NaiveTime)> = Vec::new();
    for (dia, rangos) in &body.horarios {
        let dia = DIAS
            .iter()
            .find(|d| **d == dia.as_str())
            .ok_or_else(|| AppError::BadRequest(format!("Día inválido: '{dia}'")))?;
        for r in rangos {
            let (ini, fin) = parse_rango(r)?;
            filas.push((dia, ini, fin));
        }
    }
    verificar_dueno(&state, &user, id).await?;

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM horarios_empresa WHERE empresa_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for (dia, ini, fin) in filas {
        sqlx::query("INSERT INTO horarios_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(dia)
            .bind(ini)
            .bind(fin)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(horarios_de(&state.db, id).await?))
}

#[derive(Deserialize)]
pub struct EmpresaRequest {
    nombre: String,
    direccion: String,
    telefono: Option<String>,
    slogan: Option<String>,
    duracion_turno_min: Option<i32>,
    categoria_id: i32,
    admin_id: Option<i32>,
    instagram: Option<String>,
    facebook: Option<String>,
    fotos: Option<Vec<String>>,
    /// RF-18: ausente conserva las actuales; `[]` las borra.
    etiquetas: Option<Vec<String>>,
    turnos_habilitados: Option<bool>,
    descripcion: Option<String>,
    cuit: Option<String>,
    constancia_cuit_url: Option<String>,
    verificado_estado: Option<String>,
    /// RF-04: punto elegido en el mapa; se envían juntos o ambos quedan vacíos.
    lat: Option<f64>,
    lng: Option<f64>,
    /// RF-02: canal de contacto preferido del comercio.
    modo_contacto: Option<String>,
}

const ESTADOS_VERIFICACION: [&str; 4] = ["sin_solicitar", "pendiente", "verificado", "rechazado"];
const MODOS_CONTACTO: [&str; 3] = ["llamada", "mensaje", "ambos"];

fn validar_coordenadas(lat: Option<f64>, lng: Option<f64>) -> AppResult<()> {
    match (lat, lng) {
        (None, None) => Ok(()),
        (Some(lat), Some(lng))
            if lat.is_finite()
                && lng.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lng) =>
        {
            Ok(())
        }
        (Some(_), Some(_)) => Err(AppError::BadRequest(
            "Las coordenadas están fuera de rango".into(),
        )),
        _ => Err(AppError::BadRequest(
            "Latitud y longitud deben enviarse juntas".into(),
        )),
    }
}

fn validar_modo_contacto(valor: &Option<String>) -> AppResult<Option<&str>> {
    match valor.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(v) if MODOS_CONTACTO.contains(&v) => Ok(Some(v)),
        Some(_) => Err(AppError::BadRequest(
            "El modo de contacto debe ser 'llamada', 'mensaje' o 'ambos'".into(),
        )),
    }
}

/// CUIT: se acepta con guiones o espacios y se guarda solo con sus 11 dígitos.
pub(crate) fn normalizar_cuit(valor: &Option<String>) -> AppResult<Option<String>> {
    match valor.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) => {
            let digitos: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
            if digitos.len() != 11 {
                return Err(AppError::BadRequest("El CUIT debe tener 11 dígitos".into()));
            }
            Ok(Some(digitos))
        }
    }
}

fn validar_estado_verificacion(valor: &Option<String>) -> AppResult<Option<&str>> {
    match valor.as_deref() {
        None => Ok(None),
        Some(v) if ESTADOS_VERIFICACION.contains(&v) => Ok(Some(v)),
        Some(_) => Err(AppError::BadRequest(
            "Estado de verificación inválido".into(),
        )),
    }
}

fn validar_ficha(b: &EmpresaRequest) -> AppResult<()> {
    if b.nombre.trim().is_empty() || b.direccion.trim().is_empty() {
        return Err(AppError::BadRequest(
            "Nombre y dirección son obligatorios".into(),
        ));
    }
    url_http_opt("instagram", &b.instagram)?;
    url_http_opt("facebook", &b.facebook)?;
    url_http_opt("constancia_cuit_url", &b.constancia_cuit_url)?;
    normalizar_cuit(&b.cuit)?;
    validar_estado_verificacion(&b.verificado_estado)?;
    validar_coordenadas(b.lat, b.lng)?;
    validar_modo_contacto(&b.modo_contacto)?;
    for f in b.fotos.iter().flatten() {
        url_http("foto", f)?;
    }
    Ok(())
}

/// RF-18: reemplaza las etiquetas dentro de la misma transacción que la ficha.
async fn reemplazar_etiquetas(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    empresa_id: i32,
    etiquetas: &[String],
) -> AppResult<()> {
    sqlx::query("DELETE FROM empresa_etiquetas WHERE empresa_id = $1")
        .bind(empresa_id)
        .execute(&mut **tx)
        .await?;
    for nombre in etiquetas {
        let etiqueta_id = sqlx::query_scalar::<_, i32>(
            "INSERT INTO etiquetas (nombre) VALUES ($1) \
             ON CONFLICT (nombre) DO UPDATE SET nombre = EXCLUDED.nombre RETURNING id",
        )
        .bind(nombre)
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO empresa_etiquetas (empresa_id, etiqueta_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(empresa_id)
        .bind(etiqueta_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn create_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<EmpresaRequest>,
) -> AppResult<(StatusCode, Json<Empresa>)> {
    // RF-01: solo una cuenta de comercio (admin) o de plataforma puede registrar un negocio.
    if user.rol != "admin" && user.rol != "superadmin" {
        return Err(AppError::Forbidden(
            "Solo una cuenta de comercio puede registrar un negocio".into(),
        ));
    }
    // Un admin de negocio solo puede crear una empresa a su propio nombre; solo un superadmin
    // puede asignarla a otro usuario pasando admin_id en el body.
    validar_ficha(&body)?;
    let admin_id = if user.rol == "superadmin" && body.admin_id.is_some() {
        body.admin_id
    } else {
        Some(user.id)
    };
    // Solo el superadmin puede fijar el estado de verificación en el alta; el resto nace sin solicitar.
    let cuit = normalizar_cuit(&body.cuit)?;
    let estado = if user.rol == "superadmin" {
        validar_estado_verificacion(&body.verificado_estado)?.map(str::to_string)
    } else {
        None
    };
    let verificado = estado.as_deref() == Some("verificado");
    let etiquetas = normalizar_etiquetas(&body.etiquetas)?;
    let modo_contacto = validar_modo_contacto(&body.modo_contacto)?;

    let mut tx = state.db.begin().await?;
    let id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO empresas (nombre, direccion, telefono, slogan, duracion_turno_min, categoria_id, \
         admin_id, instagram, facebook, fotos, turnos_habilitados, descripcion, \
         cuit, constancia_cuit_url, verificado, verificado_estado, lat, lng, modo_contacto) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,COALESCE($16,'sin_solicitar'),$17::numeric,$18::numeric,COALESCE($19,'ambos')) RETURNING id",
    )
    .bind(&body.nombre)
    .bind(&body.direccion)
    .bind(&body.telefono)
    .bind(&body.slogan)
    .bind(body.duracion_turno_min)
    .bind(body.categoria_id)
    .bind(admin_id)
    .bind(&body.instagram)
    .bind(&body.facebook)
    .bind(body.fotos.unwrap_or_default())
    .bind(body.turnos_habilitados.unwrap_or(true))
    .bind(&body.descripcion)
    .bind(cuit.as_deref())
    .bind(body.constancia_cuit_url.as_deref())
    .bind(verificado)
    .bind(estado.as_deref())
    .bind(body.lat)
    .bind(body.lng)
    .bind(modo_contacto)
    .fetch_one(&mut *tx)
    .await
    .map_err(fk_a_bad_request)?;
    if let Some(lista) = &etiquetas {
        reemplazar_etiquetas(&mut tx, id, lista).await?;
    }
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(fetch_empresa(&state.db, id).await?),
    ))
}

/// Un rubro inexistente es error del cliente (400), no del servidor.
fn fk_a_bad_request(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23503") => {
            AppError::BadRequest("La categoría indicada no existe".into())
        }
        _ => AppError::Db(e),
    }
}

pub(crate) async fn verificar_dueno(
    state: &AppState,
    user: &CurrentUser,
    empresa_id: i32,
) -> AppResult<()> {
    let admin_id =
        sqlx::query_scalar::<_, Option<i32>>("SELECT admin_id FROM empresas WHERE id = $1")
            .bind(empresa_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound("Empresa no encontrada".into()))?;

    if user.rol != "superadmin" && admin_id != Some(user.id) {
        return Err(AppError::Forbidden(
            "No podés editar una empresa que no es tuya".into(),
        ));
    }
    Ok(())
}

pub async fn update_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<EmpresaRequest>,
) -> AppResult<Json<Empresa>> {
    validar_ficha(&body)?;
    verificar_dueno(&state, &user, id).await?;

    // Solo un superadmin puede reasignar el dueño; para el resto, admin_id no se toca. Los campos
    // opcionales ausentes (fotos, turnos_habilitados, descripcion, etiquetas) conservan su valor.
    let nuevo_admin = if user.rol == "superadmin" {
        body.admin_id
    } else {
        None
    };
    // El CUIT lo puede cargar el dueño; el estado solo lo cambia el superadmin (y sincroniza el sello).
    let cuit = normalizar_cuit(&body.cuit)?;
    let nuevo_estado = if user.rol == "superadmin" {
        validar_estado_verificacion(&body.verificado_estado)?.map(str::to_string)
    } else {
        None
    };
    let etiquetas = normalizar_etiquetas(&body.etiquetas)?;
    let modo_contacto = validar_modo_contacto(&body.modo_contacto)?;

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE empresas SET nombre=$1, direccion=$2, telefono=$3, slogan=$4, duracion_turno_min=$5, \
         categoria_id=$6, admin_id=COALESCE($7, admin_id), instagram=$8, facebook=$9, \
         fotos=COALESCE($10, fotos), turnos_habilitados=COALESCE($11, turnos_habilitados), \
         descripcion=COALESCE($12, descripcion), cuit=COALESCE($13, cuit), \
         constancia_cuit_url=COALESCE($14, constancia_cuit_url), \
         verificado_estado=COALESCE($15, verificado_estado), \
         verificado=CASE WHEN $15 IS NULL THEN verificado ELSE ($15 = 'verificado') END, \
         modo_contacto=COALESCE($16, modo_contacto), \
         lat=COALESCE($17::numeric, lat), lng=COALESCE($18::numeric, lng) \
         WHERE id=$19",
    )
    .bind(&body.nombre)
    .bind(&body.direccion)
    .bind(&body.telefono)
    .bind(&body.slogan)
    .bind(body.duracion_turno_min)
    .bind(body.categoria_id)
    .bind(nuevo_admin)
    .bind(&body.instagram)
    .bind(&body.facebook)
    .bind(&body.fotos)
    .bind(body.turnos_habilitados)
    .bind(&body.descripcion)
    .bind(cuit.as_deref())
    .bind(body.constancia_cuit_url.as_deref())
    .bind(nuevo_estado.as_deref())
    .bind(modo_contacto)
    .bind(body.lat)
    .bind(body.lng)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(fk_a_bad_request)?;
    if let Some(lista) = &etiquetas {
        reemplazar_etiquetas(&mut tx, id, lista).await?;
    }
    tx.commit().await?;

    Ok(Json(fetch_empresa(&state.db, id).await?))
}

pub async fn delete_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    verificar_dueno(&state, &user, id).await?;
    let result = sqlx::query("DELETE FROM empresas WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;
    match result {
        Ok(r) if r.rows_affected() == 0 => Err(AppError::NotFound("Empresa no encontrada".into())),
        Ok(_) => Ok(Json(
            json!({ "message": "Empresa eliminada correctamente" }),
        )),
        // turnos.empresa_id no tiene ON DELETE CASCADE a propósito: no se borran reservas ajenas en silencio.
        Err(sqlx::Error::Database(d)) if d.code().as_deref() == Some("23503") => Err(
            AppError::Conflict("La empresa tiene turnos registrados y no puede eliminarse".into()),
        ),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize)]
pub struct TelefonoInput {
    numero: String,
    etiqueta: Option<String>,
    modo_contacto: Option<String>,
}

#[derive(Deserialize)]
pub struct TelefonosRequest {
    telefonos: Vec<TelefonoInput>,
}

pub async fn update_telefonos_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<TelefonosRequest>,
) -> AppResult<Json<Vec<Telefono>>> {
    if body.telefonos.iter().any(|t| t.numero.trim().is_empty()) {
        return Err(AppError::BadRequest(
            "Cada teléfono necesita un número".into(),
        ));
    }
    for t in &body.telefonos {
        validar_modo_contacto(&t.modo_contacto)?;
    }
    verificar_dueno(&state, &user, id).await?;

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM telefonos_empresa WHERE empresa_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for t in &body.telefonos {
        let etiqueta = t
            .etiqueta
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let modo_contacto = validar_modo_contacto(&t.modo_contacto)?;
        // ON CONFLICT: el mismo número repetido en el formulario no debe romper con un 500.
        sqlx::query(
            "INSERT INTO telefonos_empresa (empresa_id, numero, etiqueta, modo_contacto) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (empresa_id, numero) DO NOTHING",
        )
        .bind(id)
        .bind(t.numero.trim())
        .bind(etiqueta)
        .bind(modo_contacto)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    get_telefonos_by_empresa(State(state), Path(id)).await
}

// ------------------------------------------------------------------ turnos recibidos (panel del comercio)

#[derive(FromRow, Serialize)]
pub struct TurnoRecibido {
    id: i32,
    fecha: NaiveDate,
    hora: NaiveTime,
    estado: String,
    cliente_nombre: String,
}

/// Turnos próximos de un comercio, para su dueño (o un superadmin). Los cancelados no se listan.
pub async fn get_turnos_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Vec<TurnoRecibido>>> {
    verificar_dueno(&state, &user, id).await?;
    let rows = sqlx::query_as::<_, TurnoRecibido>(
        "SELECT t.id, t.fecha, t.hora, t.estado, u.nombre AS cliente_nombre          FROM turnos t JOIN usuarios u ON u.id = t.usuario_id          WHERE t.empresa_id = $1 AND t.estado <> 'cancelado' AND t.fecha >= $2          ORDER BY t.fecha, t.hora",
    )
    .bind(id)
    .bind(crate::agenda::ahora_local().date())
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

// ------------------------------------------------------------------ calificaciones (RF-13)

#[derive(Deserialize)]
pub struct CalificacionRequest {
    turno_id: i32,
    puntaje: i16,
    comentario: Option<String>,
}

pub async fn calificar_empresa(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<CalificacionRequest>,
) -> AppResult<Json<Value>> {
    if !(1..=5).contains(&body.puntaje) {
        return Err(AppError::BadRequest(
            "El puntaje debe estar entre 1 y 5".into(),
        ));
    }
    let comentario = body
        .comentario
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if comentario.is_some_and(|c| c.chars().count() > 1000) {
        return Err(AppError::BadRequest(
            "El comentario no puede superar los 1000 caracteres".into(),
        ));
    }
    let dueno = sqlx::query_scalar::<_, Option<i32>>("SELECT admin_id FROM empresas WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("Empresa no encontrada".into()))?;
    if dueno == Some(user.id) {
        return Err(AppError::Forbidden(
            "No podés calificar tu propio comercio".into(),
        ));
    }

    // RF-13: la reseña es de un turno del propio cliente, ya realizado y no cancelado.
    let turno = sqlx::query_as::<_, (i32, NaiveDate, NaiveTime, String)>(
        "SELECT empresa_id, fecha, hora, estado FROM turnos WHERE id = $1 AND usuario_id = $2",
    )
    .bind(body.turno_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("Turno no encontrado".into()))?;
    if turno.0 != id {
        return Err(AppError::BadRequest(
            "Ese turno pertenece a otro comercio".into(),
        ));
    }
    if !matches!(turno.3.as_str(), "confirmado" | "completado") {
        return Err(AppError::BadRequest(
            "No podés calificar un turno cancelado o no asistido".into(),
        ));
    }
    if turno.1.and_time(turno.2) > crate::agenda::ahora_local() {
        return Err(AppError::BadRequest(
            "Solo podés calificar después del horario del turno".into(),
        ));
    }

    sqlx::query(
        "INSERT INTO calificaciones (usuario_id, empresa_id, turno_id, puntaje, comentario) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (turno_id) WHERE turno_id IS NOT NULL \
         DO UPDATE SET puntaje = EXCLUDED.puntaje, comentario = EXCLUDED.comentario",
    )
    .bind(user.id)
    .bind(id)
    .bind(body.turno_id)
    .bind(body.puntaje)
    .bind(comentario)
    .execute(&state.db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23503") => {
            AppError::NotFound("Turno o empresa no encontrada".into())
        }
        _ => AppError::Db(e),
    })?;
    Ok(Json(json!({ "message": "Calificación registrada" })))
}

#[derive(FromRow, Serialize)]
pub struct CalificacionPublica {
    usuario_nombre: String,
    puntaje: i16,
    comentario: Option<String>,
    creado_en: DateTime<Utc>,
}

pub async fn get_calificaciones(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Vec<CalificacionPublica>>> {
    let rows = sqlx::query_as::<_, CalificacionPublica>(
        "SELECT u.nombre AS usuario_nombre, k.puntaje, k.comentario, k.creado_en \
         FROM calificaciones k JOIN usuarios u ON u.id = k.usuario_id \
         WHERE k.empresa_id = $1 ORDER BY k.creado_en DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

// ------------------------------------------------------------------ mapa (RF-07)

#[derive(FromRow, Serialize)]
pub struct PuntoMapa {
    id: i32,
    nombre: String,
    lat: f64,
    lng: f64,
    categoria_id: i32,
    verificado: bool,
}

#[derive(Deserialize)]
pub struct MapaQuery {
    categoria_id: Option<i32>,
}

/// Solo los comercios geolocalizados y solo lo mínimo para dibujar los marcadores; la ficha
/// completa se pide al abrir cada uno.
pub async fn get_mapa(
    State(state): State<AppState>,
    Query(f): Query<MapaQuery>,
) -> AppResult<Json<Vec<PuntoMapa>>> {
    let rows = sqlx::query_as::<_, PuntoMapa>(
        "SELECT e.id, e.nombre, e.lat::float8 AS lat, e.lng::float8 AS lng, e.categoria_id, e.verificado \
         FROM empresas e \
         WHERE e.lat IS NOT NULL AND e.lng IS NOT NULL \
           AND ($1::int IS NULL OR e.categoria_id = $1 OR EXISTS \
                (SELECT 1 FROM empresa_categorias ec WHERE ec.empresa_id = e.id AND ec.categoria_id = $1)) \
         ORDER BY e.id",
    )
    .bind(f.categoria_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_franjas_validas() {
        let (i, f) = parse_rango("09:00-13:30").unwrap();
        assert_eq!(
            (i.to_string(), f.to_string()),
            ("09:00:00".into(), "13:30:00".into())
        );
        assert!(parse_rango(" 08:15 - 12:00 ").is_ok());
    }

    #[test]
    fn rechaza_franjas_invalidas() {
        for malo in [
            "",
            "09:00",
            "09:00-",
            "-10:00",
            "25:00-26:00",
            "9-10",
            "10:00-09:00",
            "10:00-10:00",
            "ab-cd",
        ] {
            assert!(parse_rango(malo).is_err(), "{malo}");
        }
    }

    #[test]
    fn el_listado_no_incluye_la_descripcion_pero_la_ficha_si() {
        assert!(read_select(false, false).contains("NULL::text AS descripcion"));
        assert!(read_select(true, false).contains("e.descripcion AS descripcion"));
        assert!(read_select(false, true).contains("AS distancia_km"));
        assert!(read_select(false, false).contains("AS etiquetas"));
        assert!(!read_select(false, false).contains("e.lat::float8 - $8"));
    }

    #[test]
    fn normaliza_alias_y_deduplica_etiquetas() {
        let entrada = Some(vec![
            "wifi".into(),
            "Wi-Fi gratis".into(),
            "Acceso para sillas de ruedas".into(),
            " Terraza ".into(),
            "".into(),
        ]);
        assert_eq!(
            normalizar_etiquetas(&entrada).unwrap().unwrap(),
            ["Wi-Fi gratis", "Rampa de acceso", "Terraza"]
        );
        assert_eq!(normalizar_etiquetas(&None).unwrap(), None);
        assert_eq!(
            normalizar_etiquetas(&Some(Vec::new())).unwrap(),
            Some(Vec::new()),
            "una lista vacía borra las etiquetas"
        );
    }

    #[test]
    fn filtra_etiquetas_por_alias_sin_repetir() {
        let pedidas = Some("rampa,wifi,Wi-Fi gratis".to_string());
        assert_eq!(
            etiquetas_filtro(&pedidas),
            Some(vec!["Rampa de acceso".into(), "Wi-Fi gratis".into()])
        );
        assert_eq!(etiquetas_filtro(&Some(" , ".into())), None);
    }

    #[test]
    fn rechaza_demasiadas_etiquetas_o_nombres_largos() {
        let muchas = Some(vec!["Terraza".to_string(); MAX_ETIQUETAS + 1]);
        assert!(normalizar_etiquetas(&muchas).is_err());
        let larga = Some(vec!["x".repeat(MAX_LARGO_ETIQUETA + 1)]);
        assert!(normalizar_etiquetas(&larga).is_err());
    }
}
